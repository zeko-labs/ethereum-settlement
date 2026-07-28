// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {
    IAccessControl
} from "@openzeppelin/contracts/access/IAccessControl.sol";
import {
    ERC1967Proxy
} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

import {ISP1Verifier, ZekoSettlement} from "../src/ZekoSettlement.sol";

contract SettlementMockSP1Verifier is ISP1Verifier {
    bool public shouldRevert;

    function setShouldRevert(bool value) external {
        shouldRevert = value;
    }

    function verifyProof(
        bytes32,
        bytes calldata,
        bytes calldata
    ) external view {
        if (shouldRevert) revert("invalid proof");
    }
}

contract ZekoSettlementV1Test is Test {
    uint256 private constant PUBLIC_VALUES_LENGTH = 768;
    uint256 private constant STATE_ARRAY_LENGTH = 8;

    address private owner = address(this);
    address private alice = address(0xA11CE);
    bytes32 private programVKey = keccak256("settlement program vkey");
    bytes32 private vkHash = keccak256("zeko vk");
    bytes32 private initialActionState = keccak256("outer action before");
    bytes32[8] private initialState;

    SettlementMockSP1Verifier private sp1;
    ZekoSettlement private settlement;

    function setUp() public {
        vm.warp(1_000_000);
        for (uint256 i = 0; i < STATE_ARRAY_LENGTH; i++) {
            initialState[i] = keccak256(abi.encode("state", i));
        }
        initialState[4] = bytes32(uint256(3));
        sp1 = new SettlementMockSP1Verifier();
        settlement = _deploy();
    }

    function test_InitializeStoresCompleteState() public view {
        assertEq(address(settlement.verifier()), address(sp1));
        assertEq(settlement.programVKey(), programVKey);
        assertEq(settlement.vkHash(), vkHash);
        assertEq(settlement.actionState(), initialActionState);
        assertEq(settlement.currentRoot(), initialState[2]);
        assertEq(settlement.outerActionStateLength(), 5);
        assertEq(settlement.batchSequence(), 0);
        assertEq(settlement.currentVirtualSlot(), 10);
        _assertStateEq(settlement.outerState(), initialState);
        assertTrue(settlement.isActionStateValid(initialActionState));

        (
            uint64 index,
            uint64 acceptedAt,
            uint32 length,
            bool valid
        ) = settlement.acceptedInnerActionState(initialState[3]);
        assertEq(index, 0);
        assertEq(acceptedAt, block.timestamp);
        assertEq(length, 3);
        assertTrue(valid);
    }

    function test_DecodePublicValuesV1() public view {
        bytes32[8] memory afterState = _afterState();
        bytes memory values = _buildPublicValues(afterState);
        ZekoSettlement.DecodedPublicValues memory decoded = settlement
            .getDecodedPublicValues(values);

        assertEq(decoded.daMode, 1);
        assertEq(decoded.chainId, block.chainid);
        assertEq(decoded.settlementContract, address(settlement));
        assertEq(decoded.batchSequence, 1);
        assertEq(decoded.vkHash, vkHash);
        _assertStateEq(decoded.stateBefore, initialState);
        _assertStateEq(decoded.stateAfter, afterState);
        assertEq(decoded.outerActionStateBefore, initialActionState);
        assertEq(decoded.outerActionStateLengthBefore, 5);
        assertEq(decoded.outerActionStateLengthAfter, 6);
        assertEq(decoded.slotLower, 9);
        assertEq(decoded.slotUpper, 11);
    }

    function test_VerifyAndUpdateStoresCompleteTransition() public {
        bytes32[8] memory afterState = _afterState();
        bytes32 afterAction = keccak256("outer action after");
        bytes memory values = _buildPublicValues(afterState);

        settlement.verifyAndUpdateRoot(values, hex"1234");

        _assertStateEq(settlement.outerState(), afterState);
        assertEq(settlement.currentRoot(), afterState[2]);
        assertEq(settlement.actionState(), afterAction);
        assertEq(settlement.outerActionStateLength(), 6);
        assertEq(settlement.batchSequence(), 1);
        assertTrue(settlement.isActionStateValid(afterAction));
        (uint64 actionIndex, bool actionValid) = settlement.l2ActionStateInfo(
            afterAction
        );
        assertEq(actionIndex, 1);
        assertTrue(actionValid);

        (
            uint64 index,
            uint64 acceptedAt,
            uint32 length,
            bool valid
        ) = settlement.acceptedInnerActionState(afterState[3]);
        assertEq(index, 1);
        assertEq(acceptedAt, block.timestamp);
        assertEq(length, 4);
        assertTrue(valid);
    }

    function test_V2StoresSettlementBoundInnerActionBatch() public {
        address bridgeAddress = address(0xB12D63);
        settlement.setBridgeContract(bridgeAddress);
        bytes32 root = keccak256("inner action root");
        bytes memory values = _buildPublicValuesV2(
            _afterState(),
            bridgeAddress,
            root,
            3,
            1
        );

        settlement.verifyAndUpdateRoot(values, hex"1234");

        (
            bytes32 minaBefore,
            bytes32 minaAfter,
            bytes32 storedRoot,
            uint32 startIndex,
            uint32 count,
            uint32 commitSlotUpper,
            bool valid
        ) = settlement.innerActionBatch(1);
        assertEq(minaBefore, initialState[3]);
        assertEq(minaAfter, _afterState()[3]);
        assertEq(storedRoot, root);
        assertEq(startIndex, 3);
        assertEq(count, 1);
        assertEq(commitSlotUpper, 11);
        assertTrue(valid);
    }

    function test_V3StoresRegistryCheckpointAndExactRecord() public {
        address bridgeAddress = address(0xB12D63);
        settlement.setBridgeContract(bridgeAddress);
        bytes32 registryRoot = keccak256("Poseidon registry root");
        bytes32 recordHash = keccak256("canonical asset record");
        bytes32 recordCommitment = bytes32(uint256(991));
        bytes memory values = _buildPublicValuesV3(
            _afterState(),
            bridgeAddress,
            keccak256("inner action root"),
            3,
            1,
            registryRoot,
            1,
            1,
            recordHash,
            recordCommitment
        );

        settlement.verifyAndUpdateRoot(values, hex"1234");

        assertEq(settlement.assetRegistryRoot(), registryRoot);
        assertEq(settlement.assetRegistryCount(), 1);
        assertEq(settlement.assetRegistrySchemaVersion(), 1);
        assertTrue(settlement.settledAssetRecord(recordHash));
        assertEq(
            settlement.settledAssetRecordCommitment(recordHash),
            recordCommitment
        );
    }

    function test_V4StoresTwoRecordRegistryBatch() public {
        address bridgeAddress = address(0xB12D63);
        settlement.setBridgeContract(bridgeAddress);
        bytes32 registryRoot = keccak256("two-record Poseidon registry root");
        bytes32 recordBatchRoot = keccak256(
            "two exact canonical asset records"
        );
        bytes memory values = _buildPublicValuesV4(
            _afterState(),
            bridgeAddress,
            keccak256("inner action root"),
            3,
            1,
            registryRoot,
            2,
            1,
            recordBatchRoot,
            2
        );

        settlement.verifyAndUpdateRoot(values, hex"1234");

        assertEq(settlement.assetRegistryRoot(), registryRoot);
        assertEq(settlement.assetRegistryCount(), 2);
        assertEq(settlement.assetRegistrySchemaVersion(), 1);
        (
            bytes32 storedRegistryRoot,
            uint32 storedRegistryCount,
            uint32 storedSchema,
            bytes32 storedBatchRoot,
            uint32 storedBatchCount,
            bool valid
        ) = settlement.assetRegistryRecordBatch(1);
        assertEq(storedRegistryRoot, registryRoot);
        assertEq(storedRegistryCount, 2);
        assertEq(storedSchema, 1);
        assertEq(storedBatchRoot, recordBatchRoot);
        assertEq(storedBatchCount, 2);
        assertTrue(valid);
    }

    function test_V4RejectsRegistryBatchAboveCapacity() public {
        address bridgeAddress = address(0xB12D63);
        settlement.setBridgeContract(bridgeAddress);
        bytes32 registryRoot = keccak256("oversized Poseidon registry root");
        bytes32 recordBatchRoot = keccak256(
            "oversized exact canonical asset records"
        );
        bytes memory values = _buildPublicValuesV4(
            _afterState(),
            bridgeAddress,
            keccak256("inner action root"),
            3,
            1,
            registryRoot,
            257,
            1,
            recordBatchRoot,
            257
        );

        vm.expectRevert(
            abi.encodeWithSelector(
                ZekoSettlement.InvalidAssetRegistryBatch.selector,
                registryRoot,
                uint32(257),
                uint32(1),
                recordBatchRoot,
                uint32(257)
            )
        );
        settlement.verifyAndUpdateRoot(values, hex"1234");
    }

    function test_AppendOuterWitnessBatchRecordsExactCheckpoint() public {
        settlement.setBridgeContract(alice);
        bytes32 afterState = keccak256("witness action state");
        vm.prank(alice);
        settlement.appendOuterWitnessBatch(initialActionState, afterState, 2);

        assertEq(settlement.actionState(), afterState);
        assertEq(settlement.outerActionStateLength(), 7);
        (uint32 length, bool valid) = settlement.outerActionStateInfo(
            afterState
        );
        assertEq(length, 7);
        assertTrue(valid);
    }

    function test_RevertOnUnknownSynchronizedCheckpoint() public {
        bytes memory values = _buildPublicValues(_afterState());
        bytes32 unknown = keccak256("unknown synchronized state");
        _writeBytes32(values, 724, unknown);
        vm.expectRevert(
            abi.encodeWithSelector(
                ZekoSettlement.UnknownSynchronizedActionState.selector,
                unknown,
                uint32(5)
            )
        );
        settlement.verifyAndUpdateRoot(values, hex"01");
    }

    function test_RevertOnWrongLength() public {
        bytes memory invalid = new bytes(12);
        vm.expectRevert(
            abi.encodeWithSelector(
                ZekoSettlement.InvalidPublicValuesLength.selector,
                PUBLIC_VALUES_LENGTH,
                invalid.length
            )
        );
        settlement.getDecodedPublicValues(invalid);
    }

    function test_RevertOnWrongMagic() public {
        bytes memory values = _buildPublicValues(_afterState());
        values[0] = 0;
        vm.expectRevert(
            abi.encodeWithSelector(
                ZekoSettlement.InvalidPublicValuesMagic.selector,
                bytes4(0x004b5354)
            )
        );
        settlement.getDecodedPublicValues(values);
    }

    function test_RevertOnWrongDomain() public {
        bytes memory values = _buildPublicValues(_afterState());
        _writeUint64(values, 8, uint64(block.chainid + 1));
        vm.expectRevert(
            abi.encodeWithSelector(
                ZekoSettlement.InvalidChainId.selector,
                uint64(block.chainid),
                uint64(block.chainid + 1)
            )
        );
        settlement.verifyAndUpdateRoot(values, hex"01");
    }

    function test_RevertOnStaleOuterState() public {
        bytes memory values = _buildPublicValues(_afterState());
        bytes32 wrongRoot = keccak256("wrong root");
        _writeBytes32(values, 140 + 2 * 32, wrongRoot);
        vm.expectRevert(
            abi.encodeWithSelector(
                ZekoSettlement.InvalidOuterState.selector,
                uint256(2),
                initialState[2],
                wrongRoot
            )
        );
        settlement.verifyAndUpdateRoot(values, hex"01");
    }

    function test_RevertOnActionLengthGap() public {
        bytes memory values = _buildPublicValues(_afterState());
        _writeUint32(values, 720, 7);
        vm.expectRevert(
            abi.encodeWithSelector(
                ZekoSettlement.InvalidActionStateTransition.selector,
                uint32(5),
                uint32(7)
            )
        );
        settlement.verifyAndUpdateRoot(values, hex"01");
    }

    function test_RevertOutsideSlotRange() public {
        bytes memory values = _buildPublicValues(_afterState());
        _writeUint32(values, 760, 11);
        _writeUint32(values, 764, 12);
        vm.expectRevert(
            abi.encodeWithSelector(
                ZekoSettlement.OutsideSlotRange.selector,
                uint64(10),
                uint32(11),
                uint32(12)
            )
        );
        settlement.verifyAndUpdateRoot(values, hex"01");
    }

    function test_RevertWhenNotProver() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                IAccessControl.AccessControlUnauthorizedAccount.selector,
                alice,
                settlement.PROVER_ROLE()
            )
        );
        vm.prank(alice);
        settlement.verifyAndUpdateRoot(
            _buildPublicValues(_afterState()),
            hex"01"
        );
    }

    function test_InvalidSp1ProofStopsBeforeStateChecks() public {
        sp1.setShouldRevert(true);
        vm.expectRevert("invalid proof");
        settlement.verifyAndUpdateRoot(
            _buildPublicValues(_afterState()),
            hex"01"
        );
    }

    function test_UpgradeRole() public {
        ZekoSettlement next = new ZekoSettlement();
        vm.expectRevert(
            abi.encodeWithSelector(
                IAccessControl.AccessControlUnauthorizedAccount.selector,
                alice,
                settlement.UPGRADER_ROLE()
            )
        );
        vm.prank(alice);
        settlement.upgradeToAndCall(address(next), "");

        settlement.upgradeToAndCall(address(next), "");
        assertEq(settlement.currentRoot(), initialState[2]);
    }

    function _deploy() private returns (ZekoSettlement target) {
        ZekoSettlement implementation = new ZekoSettlement();
        ERC1967Proxy proxy = new ERC1967Proxy(
            address(implementation),
            abi.encodeCall(
                ZekoSettlement.initialize,
                (
                    owner,
                    address(sp1),
                    programVKey,
                    vkHash,
                    initialState,
                    initialActionState,
                    5,
                    uint64(block.timestamp - 100),
                    10,
                    0
                )
            )
        );
        return ZekoSettlement(address(proxy));
    }

    function _afterState() private view returns (bytes32[8] memory state) {
        state = initialState;
        state[2] = keccak256("ledger after");
        state[3] = keccak256("inner action after");
        state[4] = bytes32(uint256(4));
        state[7] = keccak256("account set after");
    }

    function _assertStateEq(
        bytes32[8] memory actual,
        bytes32[8] memory expected
    ) private pure {
        for (uint256 i = 0; i < STATE_ARRAY_LENGTH; i++) {
            assertEq(actual[i], expected[i]);
        }
    }

    function _buildPublicValues(
        bytes32[8] memory afterState
    ) private view returns (bytes memory values) {
        values = new bytes(PUBLIC_VALUES_LENGTH);
        values[0] = 0x5a;
        values[1] = 0x4b;
        values[2] = 0x53;
        values[3] = 0x54;
        _writeUint16(values, 4, 1);
        values[6] = 0x01;
        values[7] = 0;
        _writeUint64(values, 8, uint64(block.chainid));
        _writeAddress(values, 16, address(settlement));
        _writeUint64(values, 36, 1);
        _writeBytes32(values, 44, vkHash);
        _writeBytes32(values, 76, keccak256("app statement"));
        _writeBytes32(values, 108, keccak256("mina tx"));

        uint256 cursor = 140;
        for (uint256 i = 0; i < STATE_ARRAY_LENGTH; i++) {
            _writeBytes32(values, cursor, initialState[i]);
            cursor += 32;
        }
        for (uint256 i = 0; i < STATE_ARRAY_LENGTH; i++) {
            _writeBytes32(values, cursor, afterState[i]);
            cursor += 32;
        }
        _writeBytes32(values, cursor, initialActionState);
        cursor += 32;
        _writeBytes32(values, cursor, keccak256("outer action after"));
        cursor += 32;
        _writeUint32(values, cursor, 5);
        cursor += 4;
        _writeUint32(values, cursor, 6);
        cursor += 4;
        _writeBytes32(values, cursor, initialActionState);
        cursor += 32;
        _writeUint32(values, cursor, 5);
        cursor += 4;
        _writeUint32(values, cursor, 9);
        cursor += 4;
        _writeUint32(values, cursor, 11);
        cursor += 4;
        assertEq(cursor, PUBLIC_VALUES_LENGTH);
    }

    function _buildPublicValuesV2(
        bytes32[8] memory afterState,
        address bridgeAddress,
        bytes32 root,
        uint32 startIndex,
        uint32 count
    ) private view returns (bytes memory values) {
        bytes memory v1 = _buildPublicValues(afterState);
        values = new bytes(828);
        for (uint256 i = 0; i < v1.length; i++) {
            values[i] = v1[i];
        }
        _writeUint16(values, 4, 2);
        _writeAddress(values, 768, bridgeAddress);
        _writeBytes32(values, 788, root);
        _writeUint32(values, 820, startIndex);
        _writeUint32(values, 824, count);
    }

    function _buildPublicValuesV3(
        bytes32[8] memory afterState,
        address bridgeAddress,
        bytes32 innerRoot,
        uint32 startIndex,
        uint32 innerCount,
        bytes32 registryRoot,
        uint32 registryCount,
        uint32 schemaVersion,
        bytes32 recordHash,
        bytes32 recordCommitment
    ) private view returns (bytes memory values) {
        bytes memory v2 = _buildPublicValuesV2(
            afterState,
            bridgeAddress,
            innerRoot,
            startIndex,
            innerCount
        );
        values = new bytes(932);
        for (uint256 i = 0; i < v2.length; i++) {
            values[i] = v2[i];
        }
        _writeUint16(values, 4, 3);
        _writeBytes32(values, 828, registryRoot);
        _writeUint32(values, 860, registryCount);
        _writeUint32(values, 864, schemaVersion);
        _writeBytes32(values, 868, recordHash);
        _writeBytes32(values, 900, recordCommitment);
    }

    function _buildPublicValuesV4(
        bytes32[8] memory afterState,
        address bridgeAddress,
        bytes32 innerRoot,
        uint32 startIndex,
        uint32 innerCount,
        bytes32 registryRoot,
        uint32 registryCount,
        uint32 schemaVersion,
        bytes32 recordBatchRoot,
        uint32 recordBatchCount
    ) private view returns (bytes memory values) {
        bytes memory v2 = _buildPublicValuesV2(
            afterState,
            bridgeAddress,
            innerRoot,
            startIndex,
            innerCount
        );
        values = new bytes(904);
        for (uint256 i = 0; i < v2.length; i++) {
            values[i] = v2[i];
        }
        _writeUint16(values, 4, 4);
        _writeBytes32(values, 828, registryRoot);
        _writeUint32(values, 860, registryCount);
        _writeUint32(values, 864, schemaVersion);
        _writeBytes32(values, 868, recordBatchRoot);
        _writeUint32(values, 900, recordBatchCount);
    }

    function _writeAddress(
        bytes memory output,
        uint256 offset,
        address value
    ) private pure {
        for (uint256 i = 0; i < 20; i++) {
            output[offset + i] = bytes20(value)[i];
        }
    }

    function _writeBytes32(
        bytes memory output,
        uint256 offset,
        bytes32 value
    ) private pure {
        assembly {
            mstore(add(add(output, 0x20), offset), value)
        }
    }

    function _writeUint16(
        bytes memory output,
        uint256 offset,
        uint16 value
    ) private pure {
        output[offset] = bytes1(uint8(value >> 8));
        output[offset + 1] = bytes1(uint8(value));
    }

    function _writeUint32(
        bytes memory output,
        uint256 offset,
        uint32 value
    ) private pure {
        for (uint256 i = 0; i < 4; i++) {
            output[offset + i] = bytes1(uint8(value >> ((3 - i) * 8)));
        }
    }

    function _writeUint64(
        bytes memory output,
        uint256 offset,
        uint64 value
    ) private pure {
        for (uint256 i = 0; i < 8; i++) {
            output[offset + i] = bytes1(uint8(value >> ((7 - i) * 8)));
        }
    }
}
