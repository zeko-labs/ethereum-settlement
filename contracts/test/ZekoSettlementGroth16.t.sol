// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {IAccessControl} from "@openzeppelin/contracts/access/IAccessControl.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

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

        (uint64 index, uint64 acceptedAt, uint32 length, bool valid) = settlement
            .acceptedInnerActionState(initialState[3]);
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
        (uint64 actionIndex, bool actionValid) = settlement
            .l2ActionStateInfo(afterAction);
        assertEq(actionIndex, 1);
        assertTrue(actionValid);

        (uint64 index, uint64 acceptedAt, uint32 length, bool valid) = settlement
            .acceptedInnerActionState(afterState[3]);
        assertEq(index, 1);
        assertEq(acceptedAt, block.timestamp);
        assertEq(length, 4);
        assertTrue(valid);
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
        _writeBytes32(values, cursor, keccak256("synchronized outer action"));
        cursor += 32;
        _writeUint32(values, cursor, 4);
        cursor += 4;
        _writeUint32(values, cursor, 9);
        cursor += 4;
        _writeUint32(values, cursor, 11);
        cursor += 4;
        assertEq(cursor, PUBLIC_VALUES_LENGTH);
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
