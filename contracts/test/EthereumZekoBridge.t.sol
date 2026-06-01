// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

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

    function setActionStateValid(bytes32 actionState, bool valid) external {
        validActionState[actionState] = valid;
    }

    function isActionStateValid(
        bytes32 actionState
    ) external view returns (bool) {
        return validActionState[actionState];
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

    address internal owner = address(this);
    address internal alice = address(0xA11CE);
    address internal bob = address(0xB0B);

    function setUp() public {
        settlement = new MockSettlementVerifier();
        sp1Verifier = new MockSP1Verifier();
        bridge = new EthereumZekoBridge(
            owner,
            address(settlement),
            address(sp1Verifier),
            bridgeProgramVKey
        );
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
        vm.prank(alice);
        vm.expectRevert();
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

        vm.prank(alice);
        vm.expectRevert();
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

    function test_SubmitBridgeTransition_RequiresSettlementActionState()
        public
    {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        bytes32 newWithdrawState = keccak256("withdraw state");
        bytes memory publicValues = _bridgePublicValues(
            bridge.currentDepositState(),
            bridge.currentDepositState(),
            bridge.depositNonce(),
            bridge.depositNonce(),
            oldActionState,
            actionState,
            bridge.currentWithdrawState(),
            newWithdrawState,
            0,
            1
        );

        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidSettlementActionState.selector,
                actionState
            )
        );
        bridge.submitBridgeTransition(publicValues, "");
    }

    function test_SubmitBridgeTransition_StoresValidWithdrawState() public {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        bytes32 newWithdrawState = keccak256("withdraw state");
        settlement.setActionStateValid(actionState, true);
        bytes memory publicValues = _bridgePublicValues(
            bridge.currentDepositState(),
            bridge.currentDepositState(),
            bridge.depositNonce(),
            bridge.depositNonce(),
            oldActionState,
            actionState,
            bridge.currentWithdrawState(),
            newWithdrawState,
            0,
            1
        );

        bridge.submitBridgeTransition(publicValues, "");

        assertTrue(bridge.processedActionState(actionState));
        assertTrue(bridge.validWithdrawState(newWithdrawState));
        assertEq(
            bridge.withdrawStateOldActionState(newWithdrawState),
            oldActionState
        );
        assertEq(bridge.currentWithdrawState(), newWithdrawState);
    }

    function test_SubmitBridgeTransition_RevertsWhenActionStateAlreadyProcessed()
        public
    {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        bytes32 newWithdrawState = keccak256("withdraw state");
        settlement.setActionStateValid(actionState, true);
        bytes memory firstPublicValues = _bridgePublicValues(
            bridge.currentDepositState(),
            bridge.currentDepositState(),
            bridge.depositNonce(),
            bridge.depositNonce(),
            oldActionState,
            actionState,
            bridge.currentWithdrawState(),
            newWithdrawState,
            0,
            1
        );

        bridge.submitBridgeTransition(firstPublicValues, "");

        bytes memory secondPublicValues = _bridgePublicValues(
            bridge.currentDepositState(),
            bridge.currentDepositState(),
            bridge.depositNonce(),
            bridge.depositNonce(),
            oldActionState,
            actionState,
            newWithdrawState,
            keccak256("next"),
            0,
            1
        );

        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.ActionStateAlreadyProcessed.selector,
                actionState
            )
        );
        bridge.submitBridgeTransition(secondPublicValues, "");
    }

    function test_ClaimWithdraw_ReconstructsSequentialStateAndTransfersERC20()
        public
    {
        bridge.addToken(address(token18), true, 9, 18);
        token18.mint(address(bridge), 10 ether);

        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setActionStateValid(actionState, true);

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

        bridge.submitBridgeTransition(
            _bridgePublicValues(
                bridge.currentDepositState(),
                bridge.currentDepositState(),
                bridge.depositNonce(),
                bridge.depositNonce(),
                oldActionState,
                actionState,
                bytes32(0),
                state,
                0,
                3
            ),
            ""
        );

        bytes32[] memory leaves = new bytes32[](3);
        leaves[0] = leaf0;
        leaves[1] = bytes32(0);
        leaves[2] = leaf2;

        uint256 aliceBalanceBefore = token18.balanceOf(alice);

        bridge.claimWithdraw(bytes32(0), state, target, 1, leaves);

        assertEq(token18.balanceOf(alice), aliceBalanceBefore + 2 ether);
        assertEq(token18.balanceOf(address(bridge)), 8 ether);

        bytes32 nullifier = bridge.computeWithdrawNullifier(
            oldActionState,
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
        settlement.setActionStateValid(actionState, true);

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
        bridge.submitBridgeTransition(
            _bridgePublicValues(
                bridge.currentDepositState(),
                bridge.currentDepositState(),
                bridge.depositNonce(),
                bridge.depositNonce(),
                oldActionState,
                actionState,
                bytes32(0),
                state,
                0,
                1
            ),
            ""
        );

        bytes32[] memory leaves = new bytes32[](1);
        bridge.claimWithdraw(bytes32(0), state, target, 0, leaves);

        bytes32 nullifier = bridge.computeWithdrawNullifier(
            oldActionState,
            0,
            leaf
        );
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.WithdrawAlreadyClaimed.selector,
                nullifier
            )
        );
        bridge.claimWithdraw(bytes32(0), state, target, 0, leaves);
    }

    function test_ClaimWithdraw_RevertsOnWrongOrderedHashes() public {
        bridge.addToken(address(token18), true, 9, 18);
        token18.mint(address(bridge), 10 ether);

        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setActionStateValid(actionState, true);

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
        bridge.submitBridgeTransition(
            _bridgePublicValues(
                bridge.currentDepositState(),
                bridge.currentDepositState(),
                bridge.depositNonce(),
                bridge.depositNonce(),
                oldActionState,
                actionState,
                bytes32(0),
                state,
                0,
                2
            ),
            ""
        );

        bytes32[] memory leaves = new bytes32[](2);
        leaves[0] = bytes32(0);
        leaves[1] = leaf0;

        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.claimWithdraw(bytes32(0), state, target, 0, leaves);
    }

    function _bridgePublicValues(
        bytes32 ethereumStateBefore,
        bytes32 ethereumStateAfter,
        uint64 ethereumNonceBefore,
        uint64 ethereumNonceAfter,
        bytes32 zekoActionStateBefore,
        bytes32 zekoActionStateAfter,
        bytes32 ethereumWithdrawStateBefore,
        bytes32 ethereumWithdrawStateAfter,
        uint32 depositCount,
        uint32 withdrawCount
    ) private pure returns (bytes memory publicValues) {
        publicValues = new bytes(216);
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
        _writeBytes32(publicValues, cursor, ethereumWithdrawStateBefore);
        cursor += 32;
        _writeBytes32(publicValues, cursor, ethereumWithdrawStateAfter);
        cursor += 32;
        _writeUint32LE(publicValues, cursor, depositCount);
        cursor += 4;
        _writeUint32LE(publicValues, cursor, withdrawCount);
        cursor += 4;

        assert(cursor == publicValues.length);
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
