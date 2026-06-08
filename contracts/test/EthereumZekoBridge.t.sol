// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {IAccessControl} from "@openzeppelin/contracts/access/IAccessControl.sol";
import {
    ERC1967Proxy
} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

import {EthereumZekoBridge} from "../src/EthereumZekoBridge.sol";
import {ZekoAddress, ZekoAddressLib} from "../src/ZekoAddress.sol";
import {ISP1Verifier} from "../src/ZekoSettlement.sol";

contract TestERC20 is ERC20 {
    uint8 private immutable _decimals;

    constructor(
        string memory name_,
        string memory symbol_,
        uint8 decimals_
    ) ERC20(name_, symbol_) {
        _decimals = decimals_;
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }

    function decimals() public view override returns (uint8) {
        return _decimals;
    }
}

contract MockSettlementVerifier {
    mapping(bytes32 => bool) public validActionState;
    mapping(bytes32 => uint64) public l2ActionStateIndex;

    function setActionStateValid(bytes32 actionState, bool valid) external {
        validActionState[actionState] = valid;
    }

    function setL2ActionStateInfo(
        bytes32 actionState,
        uint64 index,
        bool valid
    ) external {
        l2ActionStateIndex[actionState] = index;
        validActionState[actionState] = valid;
    }

    function isActionStateValid(
        bytes32 actionState
    ) external view returns (bool) {
        return validActionState[actionState];
    }

    function l2ActionStateInfo(
        bytes32 actionState
    ) external view returns (uint64 index, bool valid) {
        return (l2ActionStateIndex[actionState], validActionState[actionState]);
    }
}

contract MockSP1Verifier is ISP1Verifier {
    bool public shouldRevert;
    bytes32 public lastProgramVKey;
    bytes public lastPublicValues;
    bytes public lastProofBytes;

    function setShouldRevert(bool value) external {
        shouldRevert = value;
    }

    function verifyProof(
        bytes32 programVKey,
        bytes calldata publicValues,
        bytes calldata proofBytes
    ) external view override {
        programVKey;
        publicValues;
        proofBytes;
        if (shouldRevert) revert("invalid proof");
    }
}

contract EthereumZekoBridgeTest is Test {
    uint256 private constant ZEKO_FIELD_ORDER =
        28948022309329048855892746252171976963363056481941560715954676764349967630337;

    EthereumZekoBridge internal bridge;
    MockSettlementVerifier internal settlement;
    MockSP1Verifier internal sp1Verifier;
    TestERC20 internal token18;
    TestERC20 internal token6;
    bytes32 internal bridgeProgramVKey = keccak256("bridge program vkey");
    bytes32 internal withdrawProgramVKey = keccak256("withdraw program vkey");

    address internal owner = address(this);
    address internal alice = address(0xA11CE);
    address internal bob = address(0xB0B);

    function setUp() public {
        settlement = new MockSettlementVerifier();
        sp1Verifier = new MockSP1Verifier();
        EthereumZekoBridge implementation = new EthereumZekoBridge();
        ERC1967Proxy proxy = new ERC1967Proxy(
            address(implementation),
            abi.encodeCall(
                EthereumZekoBridge.initialize,
                (
                    owner,
                    address(settlement),
                    address(sp1Verifier),
                    bridgeProgramVKey,
                    address(sp1Verifier),
                    withdrawProgramVKey
                )
            )
        );
        bridge = EthereumZekoBridge(payable(address(proxy)));
        token18 = new TestERC20("Token18", "TK18", 18);
        token6 = new TestERC20("Token6", "TK6", 6);

        token18.mint(alice, 100 ether);
        token6.mint(alice, 100 * 10 ** 6);
    }

    function test_SetUp_ConfiguresNativeETH() public view {
        (uint8 zekoDecimals, uint8 ethereumDecimals, bool allowed) = bridge
            .allowedToken(address(0));

        assertEq(zekoDecimals, 9);
        assertEq(ethereumDecimals, 18);
        assertTrue(allowed);
        assertEq(address(bridge.settlementVerifier()), address(settlement));
        assertEq(address(bridge.bridgeVerifier()), address(sp1Verifier));
        assertEq(bridge.bridgeProgramVKey(), bridgeProgramVKey);
        assertEq(address(bridge.withdrawVerifier()), address(sp1Verifier));
        assertEq(bridge.withdrawProgramVKey(), withdrawProgramVKey);
        assertTrue(bridge.hasRole(bridge.DEFAULT_ADMIN_ROLE(), owner));
        assertTrue(bridge.hasRole(bridge.ADMIN_ROLE(), owner));
        assertTrue(bridge.hasRole(bridge.PROVER_ROLE(), owner));
        assertTrue(bridge.hasRole(bridge.UPGRADER_ROLE(), owner));
    }

    function test_Upgrade_RevertsWhenNotUpgrader() public {
        EthereumZekoBridge newImplementation = new EthereumZekoBridge();
        bytes32 upgraderRole = bridge.UPGRADER_ROLE();

        vm.expectRevert(
            abi.encodeWithSelector(
                IAccessControl.AccessControlUnauthorizedAccount.selector,
                alice,
                upgraderRole
            )
        );
        vm.prank(alice);
        bridge.upgradeToAndCall(address(newImplementation), "");
    }

    function test_Upgrade_AllowsUpgrader() public {
        EthereumZekoBridge newImplementation = new EthereumZekoBridge();

        bridge.upgradeToAndCall(address(newImplementation), "");

        assertEq(bridge.currentDepositState(), bridge.INITIAL_DEPOSIT_STATE());
    }

    function test_AddToken_StoresDecimals() public {
        bridge.addToken(address(token18), true, 9, 18);

        (uint8 zekoDecimals, uint8 ethereumDecimals, bool allowed) = bridge
            .allowedToken(address(token18));

        assertEq(zekoDecimals, 9);
        assertEq(ethereumDecimals, 18);
        assertTrue(allowed);
    }

    function test_SetTokenAllowed_CanToggleAllowedAfterInitialization() public {
        bridge.addToken(address(token18), true, 9, 18);
        bridge.setTokenAllowed(address(token18), false);

        (uint8 zekoDecimals, uint8 ethereumDecimals, bool allowed) = bridge
            .allowedToken(address(token18));

        assertEq(zekoDecimals, 9);
        assertEq(ethereumDecimals, 18);
        assertFalse(allowed);
    }

    function test_AddToken_RevertsWhenNotOwner() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                IAccessControl.AccessControlUnauthorizedAccount.selector,
                alice,
                bridge.ADMIN_ROLE()
            )
        );
        vm.prank(alice);
        bridge.addToken(address(token18), true, 9, 18);
    }

    function test_AddToken_RevertsWhenZekoDecimalsTooHigh() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidZekoDecimals.selector,
                uint8(10)
            )
        );
        bridge.addToken(address(token18), true, 10, 18);
    }

    function test_AddToken_RevertsWhenEthereumDecimalsMismatch() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidEthereumDecimals.selector,
                address(token18),
                uint8(6),
                uint8(18)
            )
        );
        bridge.addToken(address(token18), true, 9, 6);
    }

    function test_AddToken_RevertsWhenTokenAlreadyAdded() public {
        bridge.addToken(address(token18), true, 9, 18);

        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.TokenAlreadyAdded.selector,
                address(token18)
            )
        );
        bridge.addToken(address(token18), true, 8, 18);
    }

    function test_AddToken_RevertsWhenNativeTokenAlreadyAdded() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.TokenAlreadyAdded.selector,
                address(0)
            )
        );
        bridge.addToken(address(0), true, 8, 18);
    }

    function test_SetTokenAllowed_RevertsWhenTokenNotAdded() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.TokenNotAdded.selector,
                address(token18)
            )
        );
        bridge.setTokenAllowed(address(token18), true);
    }

    function test_SetTokenAllowed_RevertsWhenNotOwner() public {
        bridge.addToken(address(token18), true, 9, 18);

        vm.expectRevert(
            abi.encodeWithSelector(
                IAccessControl.AccessControlUnauthorizedAccount.selector,
                alice,
                bridge.ADMIN_ROLE()
            )
        );
        vm.prank(alice);
        bridge.setTokenAllowed(address(token18), false);
    }

    function test_Deposit_SerializesBridgeAddressAndNormalizedAmount() public {
        bridge.addToken(address(token18), true, 9, 18);

        uint256 amount = 2 ether;
        uint64 timeout = 123456;
        ZekoAddress recipient = ZekoAddressLib.pack(0x01020304, false);

        vm.startPrank(alice);
        token18.approve(address(bridge), amount);
        (uint64 nonce, bytes32 leaf, bytes32 newState) = bridge.deposit(
            address(token18),
            amount,
            recipient,
            timeout
        );
        vm.stopPrank();

        bytes32 expectedLeaf = keccak256(
            abi.encode(
                bridge.DEPOSIT_LEAF_DOMAIN(),
                block.chainid,
                address(bridge),
                address(token18),
                recipient,
                2 * 10 ** 9,
                timeout,
                uint64(1)
            )
        );

        assertEq(nonce, 1);
        assertEq(leaf, expectedLeaf);
        assertEq(newState, bridge.currentDepositState());
        assertEq(bridge.totalDepositedByToken(address(token18)), amount);
    }

    function test_DepositETH_UsesNativeTokenConfig() public {
        uint256 amount = 3 ether;
        uint64 timeout = 777;
        ZekoAddress recipient = ZekoAddressLib.pack(0xdeadbeef, true);

        vm.deal(alice, amount);
        vm.prank(alice);
        (uint64 nonce, bytes32 leaf, bytes32 newState) = bridge.depositETH{
            value: amount
        }(recipient, timeout);

        bytes32 expectedLeaf = keccak256(
            abi.encode(
                bridge.DEPOSIT_LEAF_DOMAIN(),
                block.chainid,
                address(bridge),
                address(0),
                recipient,
                3 * 10 ** 9,
                timeout,
                uint64(1)
            )
        );

        assertEq(nonce, 1);
        assertEq(leaf, expectedLeaf);
        assertEq(newState, bridge.currentDepositState());
        assertEq(bridge.totalDepositedByToken(address(0)), amount);
        assertEq(address(bridge).balance, amount);
    }

    function test_DepositETH_RevertsWhenPrecisionDoesNotFitZekoDecimals()
        public
    {
        vm.deal(alice, 1 ether + 1);
        vm.prank(alice);
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidAmountPrecision.selector,
                address(0),
                1 ether + 1,
                uint8(18),
                uint8(9)
            )
        );
        bridge.depositETH{value: 1 ether + 1}(ZekoAddressLib.pack(1, false), 1);
    }

    function test_Deposit_RevertsWhenPrecisionDoesNotFitZekoDecimals() public {
        bridge.addToken(address(token18), true, 9, 18);

        vm.startPrank(alice);
        token18.approve(address(bridge), 1 ether + 1);
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidAmountPrecision.selector,
                address(token18),
                1 ether + 1,
                uint8(18),
                uint8(9)
            )
        );
        bridge.deposit(
            address(token18),
            1 ether + 1,
            ZekoAddressLib.pack(1, false),
            99
        );
        vm.stopPrank();
    }

    function test_Deposit_ScalesUpWhenEthereumHasFewerDecimals() public {
        bridge.addToken(address(token6), true, 9, 6);

        uint256 amount = 25 * 10 ** 6;
        uint64 timeout = 88;
        ZekoAddress recipient = ZekoAddressLib.pack(0x1234, false);

        vm.startPrank(alice);
        token6.approve(address(bridge), amount);
        (, bytes32 leaf, ) = bridge.deposit(
            address(token6),
            amount,
            recipient,
            timeout
        );
        vm.stopPrank();

        bytes32 expectedLeaf = keccak256(
            abi.encode(
                bridge.DEPOSIT_LEAF_DOMAIN(),
                block.chainid,
                address(bridge),
                address(token6),
                recipient,
                25 * 10 ** 9,
                timeout,
                uint64(1)
            )
        );

        assertEq(leaf, expectedLeaf);
    }

    function test_ComputeDepositLeaf_RevertsOnInvalidZekoAddress() public {
        ZekoAddress invalid = ZekoAddress.wrap(ZEKO_FIELD_ORDER);

        vm.expectRevert(ZekoAddressLib.InvalidZekoField.selector);
        bridge.computeDepositLeaf(address(token18), invalid, 1, 1, 1);
    }

    function test_SubmitWithdrawTransition_RequiresSettlementActionState()
        public
    {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        bytes32 newWithdrawState = keccak256("withdraw state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        bytes memory publicValues = _withdrawPublicValues(
            oldActionState,
            actionState,
            bridge.currentWithdrawState(),
            newWithdrawState,
            1
        );

        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidSettlementActionState.selector,
                actionState
            )
        );
        bridge.submitWithdrawTransition(publicValues, "");
    }

    function test_SubmitBridgeTransition_StoresProcessedDepositActionState()
        public
    {
        bytes32 oldActionState = keccak256("old deposit action state");
        bytes32 actionState = keccak256("deposit action state");
        bytes memory publicValues = _bridgePublicValues(
            bridge.currentDepositState(),
            bridge.currentDepositState(),
            bridge.depositNonce(),
            bridge.depositNonce(),
            oldActionState,
            actionState,
            0
        );

        bridge.submitBridgeTransition(publicValues, "");

        assertTrue(bridge.processedActionState(actionState));
        assertEq(bridge.currentWithdrawState(), bytes32(0));
    }

    function test_SubmitBridgeTransition_RevertsWhenNotProver() public {
        bytes32 oldActionState = keccak256("old deposit action state");
        bytes32 actionState = keccak256("deposit action state");
        bytes32 proverRole = bridge.PROVER_ROLE();
        bytes memory publicValues = _bridgePublicValues(
            bridge.currentDepositState(),
            bridge.currentDepositState(),
            bridge.depositNonce(),
            bridge.depositNonce(),
            oldActionState,
            actionState,
            0
        );

        vm.expectRevert(
            abi.encodeWithSelector(
                IAccessControl.AccessControlUnauthorizedAccount.selector,
                alice,
                proverRole
            )
        );
        vm.prank(alice);
        bridge.submitBridgeTransition(publicValues, "");
    }

    function test_SubmitWithdrawTransition_StoresWithdrawalRootInfo() public {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        bytes32 newWithdrawState = keccak256("withdraw state");
        bytes32 withdrawalRoot = keccak256("withdrawal root");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);
        bytes memory publicValues = _withdrawPublicValues(
            oldActionState,
            actionState,
            bridge.currentWithdrawState(),
            newWithdrawState,
            withdrawalRoot,
            1
        );

        bridge.submitWithdrawTransition(publicValues, "");

        assertTrue(bridge.processedActionState(actionState));
        (
            bytes32 storedWithdrawalRoot,
            bytes32 storedStateBefore,
            bytes32 storedStateAfter,
            uint64 storedOldActionStateIndex,
            uint32 storedWithdrawCount,
            bool valid
        ) = bridge.withdrawalRootInfo(oldActionState);
        assertEq(storedWithdrawalRoot, withdrawalRoot);
        assertEq(storedStateBefore, bytes32(0));
        assertEq(storedStateAfter, newWithdrawState);
        assertEq(storedOldActionStateIndex, 0);
        assertEq(storedWithdrawCount, 1);
        assertTrue(valid);
        assertEq(bridge.currentWithdrawState(), newWithdrawState);
    }

    function test_SubmitWithdrawTransition_RevertsWhenL2ActionStateSkipsIndex()
        public
    {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        bytes32 newWithdrawState = keccak256("withdraw state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 2, true);
        bytes memory publicValues = _withdrawPublicValues(
            oldActionState,
            actionState,
            bridge.currentWithdrawState(),
            newWithdrawState,
            1
        );

        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidL2ActionStateTransition.selector,
                oldActionState,
                actionState
            )
        );
        bridge.submitWithdrawTransition(publicValues, "");
    }

    function test_SubmitWithdrawTransition_RevertsWhenActionStateAlreadyProcessed()
        public
    {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        bytes32 newWithdrawState = keccak256("withdraw state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);
        bytes memory firstPublicValues = _withdrawPublicValues(
            oldActionState,
            actionState,
            bridge.currentWithdrawState(),
            newWithdrawState,
            1
        );

        bridge.submitWithdrawTransition(firstPublicValues, "");

        bytes memory secondPublicValues = _withdrawPublicValues(
            oldActionState,
            actionState,
            newWithdrawState,
            keccak256("next"),
            1
        );

        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.ActionStateAlreadyProcessed.selector,
                actionState
            )
        );
        bridge.submitWithdrawTransition(secondPublicValues, "");
    }

    function test_ClaimWithdraw_VerifiesMerkleProofAndTransfersERC20() public {
        bridge.addToken(address(token18), true, 9, 18);
        token18.mint(address(bridge), 10 ether);

        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);

        EthereumZekoBridge.WithdrawClaim memory target = EthereumZekoBridge
            .WithdrawClaim({
                token: _addressField(address(token18)),
                recipient: _addressField(alice),
                amount: bytes32(uint256(2 * 10 ** 9))
            });

        bytes32 leaf0 = bridge.computeWithdrawLeaf(
            _addressField(address(token18)),
            _addressField(bob),
            bytes32(uint256(1 * 10 ** 9))
        );
        bytes32 leaf1 = bridge.computeWithdrawLeaf(
            target.token,
            target.recipient,
            target.amount
        );
        bytes32 leaf2 = bridge.computeWithdrawLeaf(
            _addressField(address(token18)),
            _addressField(address(0xCAFE)),
            bytes32(uint256(3 * 10 ** 9))
        );

        bytes32 state = bytes32(0);
        state = bridge.computeNextWithdrawState(state, leaf0);
        state = bridge.computeNextWithdrawState(state, leaf1);
        state = bridge.computeNextWithdrawState(state, leaf2);

        bytes32[] memory leaves = new bytes32[](3);
        leaves[0] = leaf0;
        leaves[1] = leaf1;
        leaves[2] = leaf2;
        bytes32 withdrawalRoot = _merkleRoot(leaves);

        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                oldActionState,
                actionState,
                bytes32(0),
                state,
                withdrawalRoot,
                3
            ),
            ""
        );

        uint256 aliceBalanceBefore = token18.balanceOf(alice);

        bridge.claimWithdraw(
            oldActionState,
            target,
            1,
            _merkleProof(leaves, 1)
        );

        assertEq(token18.balanceOf(alice), aliceBalanceBefore + 2 ether);
        assertEq(token18.balanceOf(address(bridge)), 8 ether);

        bytes32 nullifier = bridge.computeWithdrawNullifier(
            0,
            1,
            leaf1
        );
        assertTrue(bridge.spentWithdraw(nullifier));
    }

    function test_ClaimWithdraw_RevertsOnDoubleClaim() public {
        bridge.addToken(address(token18), true, 9, 18);
        token18.mint(address(bridge), 10 ether);

        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);

        EthereumZekoBridge.WithdrawClaim memory target = EthereumZekoBridge
            .WithdrawClaim({
                token: _addressField(address(token18)),
                recipient: _addressField(alice),
                amount: bytes32(uint256(2 * 10 ** 9))
            });

        bytes32 leaf = bridge.computeWithdrawLeaf(
            target.token,
            target.recipient,
            target.amount
        );
        bytes32 state = bridge.computeNextWithdrawState(bytes32(0), leaf);
        bytes32[] memory leaves = new bytes32[](1);
        leaves[0] = leaf;
        bytes32 withdrawalRoot = _merkleRoot(leaves);
        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                oldActionState,
                actionState,
                bytes32(0),
                state,
                withdrawalRoot,
                1
            ),
            ""
        );

        bytes32[16] memory proof = _merkleProof(leaves, 0);
        bridge.claimWithdraw(oldActionState, target, 0, proof);

        bytes32 nullifier = bridge.computeWithdrawNullifier(
            0,
            0,
            leaf
        );
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.WithdrawAlreadyClaimed.selector,
                nullifier
            )
        );
        bridge.claimWithdraw(oldActionState, target, 0, proof);
    }

    function test_ClaimWithdraw_RevertsOnInvalidMerkleProof() public {
        bridge.addToken(address(token18), true, 9, 18);
        token18.mint(address(bridge), 10 ether);

        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);

        EthereumZekoBridge.WithdrawClaim memory target = EthereumZekoBridge
            .WithdrawClaim({
                token: _addressField(address(token18)),
                recipient: _addressField(alice),
                amount: bytes32(uint256(2 * 10 ** 9))
            });

        bytes32 leaf0 = bridge.computeWithdrawLeaf(
            _addressField(address(token18)),
            _addressField(bob),
            bytes32(uint256(1 * 10 ** 9))
        );
        bytes32 leaf1 = bridge.computeWithdrawLeaf(
            target.token,
            target.recipient,
            target.amount
        );

        bytes32 state = bridge.computeNextWithdrawState(bytes32(0), leaf0);
        state = bridge.computeNextWithdrawState(state, leaf1);
        bytes32[] memory leaves = new bytes32[](2);
        leaves[0] = leaf0;
        leaves[1] = leaf1;
        bytes32 withdrawalRoot = _merkleRoot(leaves);
        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                oldActionState,
                actionState,
                bytes32(0),
                state,
                withdrawalRoot,
                2
            ),
            ""
        );

        bytes32[16] memory proof = _merkleProof(leaves, 1);
        proof[0] = keccak256("invalid sibling");

        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.claimWithdraw(oldActionState, target, 1, proof);
    }

    function test_ClaimWithdraw_RevertsWhenIndexExceedsWithdrawCount() public {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);

        EthereumZekoBridge.WithdrawClaim memory target = EthereumZekoBridge
            .WithdrawClaim({
                token: bytes32(0),
                recipient: _addressField(alice),
                amount: bytes32(uint256(1))
            });
        bytes32 leaf = bridge.computeWithdrawLeaf(
            target.token,
            target.recipient,
            target.amount
        );
        bytes32[] memory leaves = new bytes32[](1);
        leaves[0] = leaf;
        bytes32 withdrawalRoot = _merkleRoot(leaves);

        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                oldActionState,
                actionState,
                bytes32(0),
                bridge.computeNextWithdrawState(bytes32(0), leaf),
                withdrawalRoot,
                1
            ),
            ""
        );

        (, , , , uint32 withdrawCount, bool valid) = bridge
            .withdrawalRootInfo(oldActionState);
        assertEq(withdrawCount, 1);
        assertTrue(valid);
        bytes32[16] memory proof = _merkleProof(leaves, 0);

        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.claimWithdraw(oldActionState, target, 1, proof);
    }

    function test_ClaimWithdraw_RevertsForUnknownOldActionState() public {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);

        EthereumZekoBridge.WithdrawClaim memory target = EthereumZekoBridge
            .WithdrawClaim({
                token: bytes32(0),
                recipient: _addressField(alice),
                amount: bytes32(uint256(1))
            });
        bytes32 leaf = bridge.computeWithdrawLeaf(
            target.token,
            target.recipient,
            target.amount
        );
        bytes32[] memory leaves = new bytes32[](1);
        leaves[0] = leaf;
        bytes32 withdrawalRoot = _merkleRoot(leaves);

        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                oldActionState,
                actionState,
                bytes32(0),
                bridge.computeNextWithdrawState(bytes32(0), leaf),
                withdrawalRoot,
                1
            ),
            ""
        );

        bytes32[16] memory proof = _merkleProof(leaves, 0);
        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.claimWithdraw(
            keccak256("wrong old action state"),
            target,
            0,
            proof
        );
    }

    function test_SubmitWithdrawTransition_RevertsAboveMaxWithdrawCount()
        public
    {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);
        uint32 invalidWithdrawCount = uint32(
            bridge.MAX_WITHDRAW_COUNT() + 1
        );

        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                oldActionState,
                actionState,
                bytes32(0),
                keccak256("withdraw state"),
                keccak256("withdrawal root"),
                invalidWithdrawCount
            ),
            ""
        );
    }

    function test_SubmitWithdrawTransition_RevertsOnZeroRootForNonEmptyBatch()
        public
    {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);

        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                oldActionState,
                actionState,
                bytes32(0),
                keccak256("withdraw state"),
                bytes32(0),
                1
            ),
            ""
        );
    }

    function test_SubmitWithdrawTransition_AllowsSameRootForDifferentActionStates()
        public
    {
        bytes32 firstActionState = keccak256("first action state");
        bytes32 secondActionState = keccak256("second action state");
        bytes32 thirdActionState = keccak256("third action state");
        bytes32 firstWithdrawState = keccak256("first withdraw state");
        bytes32 secondWithdrawState = keccak256("second withdraw state");
        bytes32 withdrawalRoot = keccak256("withdrawal root");
        settlement.setL2ActionStateInfo(firstActionState, 0, true);
        settlement.setL2ActionStateInfo(secondActionState, 1, true);
        settlement.setL2ActionStateInfo(thirdActionState, 2, true);

        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                firstActionState,
                secondActionState,
                bytes32(0),
                firstWithdrawState,
                withdrawalRoot,
                1
            ),
            ""
        );

        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                secondActionState,
                thirdActionState,
                firstWithdrawState,
                secondWithdrawState,
                withdrawalRoot,
                1
            ),
            ""
        );

        (bytes32 firstStoredRoot, , , , , bool firstValid) = bridge
            .withdrawalRootInfo(firstActionState);
        (bytes32 secondStoredRoot, , , , , bool secondValid) = bridge
            .withdrawalRootInfo(secondActionState);
        assertEq(firstStoredRoot, withdrawalRoot);
        assertEq(secondStoredRoot, withdrawalRoot);
        assertTrue(firstValid);
        assertTrue(secondValid);
    }

    function test_DecodeWithdrawPublicValues_Expects164Bytes() public view {
        bytes32 withdrawalRoot = keccak256("withdrawal root");
        EthereumZekoBridge.DecodedWithdrawPublicValues memory decoded = bridge
            .decodeWithdrawPublicValues(
                _withdrawPublicValues(
                    bytes32(uint256(1)),
                    bytes32(uint256(2)),
                    bytes32(uint256(3)),
                    bytes32(uint256(4)),
                    withdrawalRoot,
                    5
                )
            );

        assertEq(decoded.withdrawalRoot, withdrawalRoot);
        assertEq(decoded.withdrawCount, 5);
    }

    function test_DecodeWithdrawPublicValues_RevertsOnOld132Bytes() public {
        bytes memory oldPublicValues = new bytes(132);
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidBridgePublicValuesLength.selector,
                uint256(164),
                uint256(132)
            )
        );
        bridge.decodeWithdrawPublicValues(oldPublicValues);
    }

    function _bridgePublicValues(
        bytes32 ethereumStateBefore,
        bytes32 ethereumStateAfter,
        uint64 ethereumNonceBefore,
        uint64 ethereumNonceAfter,
        bytes32 zekoActionStateBefore,
        bytes32 zekoActionStateAfter,
        uint32 depositCount
    ) private pure returns (bytes memory publicValues) {
        publicValues = new bytes(148);
        uint256 cursor = 0;

        _writeBytes32(publicValues, cursor, ethereumStateBefore);
        cursor += 32;
        _writeBytes32(publicValues, cursor, ethereumStateAfter);
        cursor += 32;
        _writeUint64LE(publicValues, cursor, ethereumNonceBefore);
        cursor += 8;
        _writeUint64LE(publicValues, cursor, ethereumNonceAfter);
        cursor += 8;
        _writeBytes32(publicValues, cursor, zekoActionStateBefore);
        cursor += 32;
        _writeBytes32(publicValues, cursor, zekoActionStateAfter);
        cursor += 32;
        _writeUint32LE(publicValues, cursor, depositCount);
        cursor += 4;
        assert(cursor == publicValues.length);
    }

    function _withdrawPublicValues(
        bytes32 zekoActionStateBefore,
        bytes32 zekoActionStateAfter,
        bytes32 ethereumWithdrawStateBefore,
        bytes32 ethereumWithdrawStateAfter,
        uint32 withdrawCount
    ) private pure returns (bytes memory publicValues) {
        return
            _withdrawPublicValues(
                zekoActionStateBefore,
                zekoActionStateAfter,
                ethereumWithdrawStateBefore,
                ethereumWithdrawStateAfter,
                keccak256(
                    abi.encode(
                        "test withdrawal root",
                        ethereumWithdrawStateAfter
                    )
                ),
                withdrawCount
            );
    }

    function _withdrawPublicValues(
        bytes32 zekoActionStateBefore,
        bytes32 zekoActionStateAfter,
        bytes32 ethereumWithdrawStateBefore,
        bytes32 ethereumWithdrawStateAfter,
        bytes32 withdrawalRoot,
        uint32 withdrawCount
    ) private pure returns (bytes memory publicValues) {
        publicValues = new bytes(164);
        uint256 cursor = 0;

        _writeBytes32(publicValues, cursor, zekoActionStateBefore);
        cursor += 32;
        _writeBytes32(publicValues, cursor, zekoActionStateAfter);
        cursor += 32;
        _writeBytes32(publicValues, cursor, ethereumWithdrawStateBefore);
        cursor += 32;
        _writeBytes32(publicValues, cursor, ethereumWithdrawStateAfter);
        cursor += 32;
        _writeBytes32(publicValues, cursor, withdrawalRoot);
        cursor += 32;
        _writeUint32LE(publicValues, cursor, withdrawCount);
        cursor += 4;

        assert(cursor == publicValues.length);
    }

    function _merkleRoot(
        bytes32[] memory leaves
    ) private view returns (bytes32) {
        if (leaves.length == 0) return _zeroHashes()[16];

        bytes32[17] memory zeroHashes = _zeroHashes();
        bytes32[] memory nodes = leaves;

        for (uint256 level = 0; level < 16; level++) {
            bytes32[] memory parents = new bytes32[]((nodes.length + 1) / 2);
            for (uint256 i = 0; i < nodes.length; i += 2) {
                bytes32 right = i + 1 < nodes.length
                    ? nodes[i + 1]
                    : zeroHashes[level];
                parents[i / 2] = _hashMerkleNode(nodes[i], right);
            }
            nodes = parents;
        }

        return nodes[0];
    }

    function _merkleProof(
        bytes32[] memory leaves,
        uint256 targetIndex
    ) private view returns (bytes32[16] memory proof) {
        bytes32[17] memory zeroHashes = _zeroHashes();
        bytes32[] memory nodes = leaves;
        uint256 index = targetIndex;

        for (uint256 level = 0; level < 16; level++) {
            uint256 siblingIndex = index ^ 1;
            proof[level] = siblingIndex < nodes.length
                ? nodes[siblingIndex]
                : zeroHashes[level];

            bytes32[] memory parents = new bytes32[]((nodes.length + 1) / 2);
            for (uint256 i = 0; i < nodes.length; i += 2) {
                bytes32 right = i + 1 < nodes.length
                    ? nodes[i + 1]
                    : zeroHashes[level];
                parents[i / 2] = _hashMerkleNode(nodes[i], right);
            }
            nodes = parents;
            index >>= 1;
        }
    }

    function _zeroHashes() private view returns (bytes32[17] memory hashes) {
        for (uint256 level = 0; level < 16; level++) {
            hashes[level + 1] = _hashMerkleNode(
                hashes[level],
                hashes[level]
            );
        }
    }

    function _hashMerkleNode(
        bytes32 left,
        bytes32 right
    ) private view returns (bytes32) {
        return
            keccak256(
                abi.encode(bridge.WITHDRAW_MERKLE_NODE_DOMAIN(), left, right)
            );
    }

    function _writeBytes32(
        bytes memory data,
        uint256 offset,
        bytes32 value
    ) private pure {
        assembly {
            mstore(add(add(data, 0x20), offset), value)
        }
    }

    function _writeUint64LE(
        bytes memory data,
        uint256 offset,
        uint64 value
    ) private pure {
        for (uint256 i = 0; i < 8; i++) {
            data[offset + i] = bytes1(uint8(value >> (8 * i)));
        }
    }

    function _writeUint32LE(
        bytes memory data,
        uint256 offset,
        uint32 value
    ) private pure {
        for (uint256 i = 0; i < 4; i++) {
            data[offset + i] = bytes1(uint8(value >> (8 * i)));
        }
    }

    function _addressField(address value) private pure returns (bytes32) {
        return bytes32(uint256(uint160(value)));
    }
}
