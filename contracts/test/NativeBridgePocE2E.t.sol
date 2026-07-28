// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

import {EthereumZekoBridge} from "../src/EthereumZekoBridge.sol";
import {ZekoAssetRegistry} from "../src/ZekoAssetRegistry.sol";
import {ZekoAddress, ZekoAddressLib} from "../src/ZekoAddress.sol";
import {ISP1Verifier, ZekoSettlement} from "../src/ZekoSettlement.sol";

contract NativeBridgePocMockVerifier is ISP1Verifier {
    function verifyProof(bytes32, bytes calldata, bytes calldata) external pure {}
}

contract ERC20BridgePocToken is ERC20 {
    constructor() ERC20("Bridge PoC Token", "BPT") {}

    function mint(address recipient, uint256 amount) external {
        _mint(recipient, amount);
    }
}

/// @dev Contract/glue checkpoint. Pickles and SP1 execution are covered by
/// their native suites; this test uses a mock verifier to exercise the complete
/// on-chain custody and checkpoint flow without generating an SP1 proof.
contract NativeBridgePocE2ETest is Test {
    bytes32 private constant VK_HASH = keccak256("zeko bridge poc vk");
    bytes32 private constant PROGRAM_VKEY = keccak256("settlement program");
    bytes32 private constant BRIDGE_VKEY = keccak256("bridge program");

    NativeBridgePocMockVerifier private verifier;
    ZekoSettlement private settlement;
    EthereumZekoBridge private bridge;
    bytes32[8] private initialState;
    bytes32 private initialOuterActionState;

    address private depositor = address(0xA11CE);
    address private recipient = address(0xB0B);

    function setUp() public {
        vm.warp(1_000_000);
        verifier = new NativeBridgePocMockVerifier();
        initialOuterActionState = bytes32(uint256(123));
        for (uint256 i = 0; i < 8; i++) {
            initialState[i] = keccak256(abi.encode("initial outer state", i));
        }
        initialState[4] = bytes32(0);

        ZekoSettlement settlementImplementation = new ZekoSettlement();
        settlement = ZekoSettlement(
            address(
                new ERC1967Proxy(
                    address(settlementImplementation),
                    abi.encodeCall(
                        ZekoSettlement.initialize,
                        (
                            address(this),
                            address(verifier),
                            PROGRAM_VKEY,
                            VK_HASH,
                            initialState,
                            initialOuterActionState,
                            uint32(0),
                            uint64(block.timestamp - 100),
                            uint32(10),
                            uint32(0)
                        )
                    )
                )
            )
        );

        EthereumZekoBridge bridgeImplementation = new EthereumZekoBridge(new ZekoAssetRegistry());
        bridge = EthereumZekoBridge(
            payable(address(
                    new ERC1967Proxy(
                        address(bridgeImplementation),
                        abi.encodeCall(
                            EthereumZekoBridge.initialize,
                            (
                                address(this),
                                address(settlement),
                                address(verifier),
                                BRIDGE_VKEY,
                                address(verifier),
                                bytes32(0)
                            )
                        )
                    )
                ))
        );
        settlement.setBridgeContract(address(bridge));
    }

    function test_NativeDepositToDelayedWithdrawal() public {
        vm.deal(depositor, 1 ether);
        ZekoAddress zekoRecipient = ZekoAddressLib.pack(0x1234, false);
        vm.prank(depositor);
        bridge.depositETH{value: 1 ether}(zekoRecipient);

        bytes32 witnessActionState = bytes32(uint256(456));
        bridge.submitBridgeTransition(_bridgeReceipt(witnessActionState), "");
        assertEq(settlement.actionState(), witnessActionState);
        assertEq(settlement.outerActionStateLength(), 1);
        assertEq(bridge.bridgedDepositNonce(), 1);

        uint64 zekoAmount = 1_000_000_000;
        bytes32 actionFieldsHash = keccak256("real inner action fields");
        bytes32 withdrawalLeaf = bridge.computeNativeWithdrawalLeaf(0, recipient, zekoAmount, actionFieldsHash);
        (bytes32 innerActionRoot, bytes32[16] memory withdrawalProof) = _singleLeafTree(withdrawalLeaf);
        settlement.verifyAndUpdateRoot(
            _settlementReceipt(witnessActionState, keccak256("commit action state"), innerActionRoot), ""
        );

        (,, bytes32 storedRoot, uint32 start, uint32 count,, bool valid) = settlement.innerActionBatch(1);
        assertTrue(valid);
        assertEq(storedRoot, innerActionRoot);
        assertEq(start, 0);
        assertEq(count, 1);

        uint64 claimableSlot = settlement.currentVirtualSlot() + 20;
        vm.expectRevert(
            abi.encodeWithSelector(EthereumZekoBridge.WithdrawalNotYetClaimable.selector, uint64(10), claimableSlot)
        );
        bridge.claimNativeWithdrawal(1, 0, recipient, zekoAmount, actionFieldsHash, withdrawalProof);

        vm.warp(block.timestamp + 200);
        uint256 beforeBalance = recipient.balance;
        bridge.claimNativeWithdrawal(1, 0, recipient, zekoAmount, actionFieldsHash, withdrawalProof);
        assertEq(recipient.balance - beforeBalance, 1 ether);
        assertEq(bridge.nativeEscrowLiability(), 0);
        assertEq(bridge.nextWithdrawalIndex(recipient), 1);
    }

    function test_ERC20DepositWitnessToDelayedAssetWithdrawal() public {
        bridge.setLegacyDepositEnabled(true);
        bridge.setLegacyWithdrawEnabled(true);
        ERC20BridgePocToken token = new ERC20BridgePocToken();
        bytes32 tokenOwner = bytes32(uint256(0x123456));
        bytes32 tokenId = keccak256("wrapped BPT token id");
        bridge.registerToken(address(token), tokenOwner, tokenId, 18, 18, type(uint64).max);

        uint64 amount = 2 ether;
        token.mint(depositor, amount);
        vm.startPrank(depositor);
        token.approve(address(bridge), amount);
        bridge.submitDeposit(address(token), amount, ZekoAddressLib.pack(0x1234, false));
        vm.stopPrank();

        assertEq(token.balanceOf(address(bridge)), amount);
        assertEq(bridge.escrowLiabilityByToken(address(token)), amount);

        bytes32 witnessActionState = bytes32(uint256(456));
        bridge.submitBridgeTransition(_bridgeReceipt(witnessActionState), "");
        assertEq(settlement.actionState(), witnessActionState);
        assertEq(bridge.bridgedDepositNonce(), 1);

        bytes32 actionFieldsHash = keccak256("real asset-bound inner action fields");
        bytes32 withdrawalLeaf = bridge.computeLegacyERC20WithdrawalLeaf(
            0, address(token), bridge.assetIdByToken(address(token)), recipient, amount, actionFieldsHash
        );
        (bytes32 innerActionRoot, bytes32[16] memory withdrawalProof) = _singleLeafTree(withdrawalLeaf);
        settlement.verifyAndUpdateRoot(
            _settlementReceipt(witnessActionState, keccak256("commit action state"), innerActionRoot), ""
        );

        uint64 claimableSlot = settlement.currentVirtualSlot() + 20;
        vm.expectRevert(
            abi.encodeWithSelector(EthereumZekoBridge.WithdrawalNotYetClaimable.selector, uint64(10), claimableSlot)
        );
        bridge.claimERC20Withdrawal(1, 0, address(token), recipient, amount, actionFieldsHash, withdrawalProof);

        vm.warp(block.timestamp + 200);
        bridge.claimERC20Withdrawal(1, 0, address(token), recipient, amount, actionFieldsHash, withdrawalProof);
        assertEq(token.balanceOf(recipient), amount);
        assertEq(bridge.escrowLiabilityByToken(address(token)), 0);
        assertEq(bridge.nextTokenWithdrawalIndex(address(token), recipient), 1);
    }

    function _bridgeReceipt(bytes32 witnessActionState) private view returns (bytes memory) {
        // The mock verifier treats this as the Poseidon aux already checked by
        // SP1. Rust/OCaml vector tests cover that proof-side calculation.
        bytes32 aux = keccak256("mock SP1-proven deposit aux");
        bytes memory action = abi.encodePacked(
            bytes32(uint256(1)), aux, bytes32(0), bytes32(0), bytes32(uint256(type(uint32).max)), witnessActionState
        );
        return abi.encodePacked(
            bytes4(0x5a4b4252),
            uint16(2),
            uint16(0),
            bridge.depositStateByNonce(0),
            bridge.currentDepositState(),
            uint64(0),
            uint64(1),
            initialOuterActionState,
            witnessActionState,
            uint32(0),
            uint32(1),
            uint32(1),
            action
        );
    }

    function _settlementReceipt(bytes32 witnessActionState, bytes32 commitActionState, bytes32 innerActionRoot)
        private
        view
        returns (bytes memory values)
    {
        bytes32[8] memory afterState = initialState;
        afterState[2] = keccak256("ledger after withdrawal");
        afterState[3] = keccak256("inner action state after withdrawal");
        afterState[4] = bytes32(uint256(1));

        values = new bytes(828);
        values[0] = 0x5a;
        values[1] = 0x4b;
        values[2] = 0x53;
        values[3] = 0x54;
        _writeUint16(values, 4, 2);
        values[6] = bytes1(uint8(1));
        _writeUint64(values, 8, uint64(block.chainid));
        _writeAddress(values, 16, address(settlement));
        _writeUint64(values, 36, 1);
        _writeBytes32(values, 44, VK_HASH);
        _writeBytes32(values, 76, keccak256("app statement"));
        _writeBytes32(values, 108, keccak256("mina transaction"));

        uint256 cursor = 140;
        for (uint256 i = 0; i < 8; i++) {
            _writeBytes32(values, cursor, initialState[i]);
            cursor += 32;
        }
        for (uint256 i = 0; i < 8; i++) {
            _writeBytes32(values, cursor, afterState[i]);
            cursor += 32;
        }
        _writeBytes32(values, cursor, witnessActionState);
        cursor += 32;
        _writeBytes32(values, cursor, commitActionState);
        cursor += 32;
        _writeUint32(values, cursor, 1);
        cursor += 4;
        _writeUint32(values, cursor, 2);
        cursor += 4;
        _writeBytes32(values, cursor, witnessActionState);
        cursor += 32;
        _writeUint32(values, cursor, 1);
        cursor += 4;
        _writeUint32(values, cursor, 10);
        cursor += 4;
        _writeUint32(values, cursor, 10);
        cursor += 4;
        assertEq(cursor, 768);
        _writeAddress(values, 768, address(bridge));
        _writeBytes32(values, 788, innerActionRoot);
        _writeUint32(values, 820, 0);
        _writeUint32(values, 824, 1);
    }

    function _singleLeafTree(bytes32 leaf) private view returns (bytes32 root, bytes32[16] memory proof) {
        root = leaf;
        bytes32 zero;
        for (uint256 level = 0; level < 16; level++) {
            proof[level] = zero;
            root = keccak256(abi.encode(bridge.INNER_ACTION_NODE_V2_DOMAIN(), root, zero));
            zero = keccak256(abi.encode(bridge.INNER_ACTION_NODE_V2_DOMAIN(), zero, zero));
        }
    }

    function _writeAddress(bytes memory output, uint256 offset, address value) private pure {
        for (uint256 i = 0; i < 20; i++) {
            output[offset + i] = bytes20(value)[i];
        }
    }

    function _writeBytes32(bytes memory output, uint256 offset, bytes32 value) private pure {
        assembly {
            mstore(add(add(output, 0x20), offset), value)
        }
    }

    function _writeUint16(bytes memory output, uint256 offset, uint16 value) private pure {
        bytes2 encoded = bytes2(value);
        output[offset] = encoded[0];
        output[offset + 1] = encoded[1];
    }

    function _writeUint32(bytes memory output, uint256 offset, uint32 value) private pure {
        bytes4 encoded = bytes4(value);
        for (uint256 i = 0; i < 4; i++) {
            output[offset + i] = encoded[i];
        }
    }

    function _writeUint64(bytes memory output, uint256 offset, uint64 value) private pure {
        bytes8 encoded = bytes8(value);
        for (uint256 i = 0; i < 8; i++) {
            output[offset + i] = encoded[i];
        }
    }
}
