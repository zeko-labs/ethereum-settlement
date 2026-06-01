// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test, console2} from "forge-std/Test.sol";
import {stdJson} from "forge-std/StdJson.sol";

import {ZekoProofVerifier} from "../src/ZekoProofVerifier.sol";
import {ISP1Verifier} from "../src/ZekoProofVerifier.sol";

contract ZekoProofVerifierGroth16Test is Test {
    using stdJson for string;

    uint256 private constant PUBLIC_VALUES_LENGTH = 577;
    uint256 private constant STATE_ARRAY_LENGTH = 8;

    address private constant SP1_GATEWAY =
        0x397A5f7f3dBd538f23DE225B51f532c34448dA9B;

    // Current Zeko state — these live inside the SP1 public values, not the SP1 vkey.
    //
    // vk_hash          = Hash of the current Zeko verification key
    // state_before[3]  = current account tree root
    // action_state     = current action state hash
    bytes32 private constant VK_HASH =
        bytes32(
            uint256(
                28938888072174442574591380326671967812369207887553320784822775940404701845190
            )
        );
    bytes32 private constant CURRENT_ROOT =
        bytes32(
            uint256(
                11066481997049907237147074214507440714257448164444404179272910777489391657254
            )
        );
    bytes32 private constant ACTION_STATE =
        bytes32(
            uint256(
                24329566355992902769881875375733216652114605558452463596572110463644683476021
            )
        );

    address public owner = address(this);
    address public alice = address(0xA11CE);
    address public bob = address(0xB0B);

    ISP1Verifier public gateway;
    ZekoProofVerifier public zeko;

    // SP1 program vkey — only needed for the real proof test.
    // Loaded from the fixture file which must be regenerated when the Zeko VK changes.
    bytes32 public fixtureVkey;
    bytes public fixturePublicValues;
    bytes public fixtureProof;

    // -------------------------------------------------------------------------
    // Setup
    // -------------------------------------------------------------------------

    function setUp() public {
        vm.createSelectFork("mainnet");
        gateway = ISP1Verifier(SP1_GATEWAY);

        // Load the SP1 program vkey from the fixture.
        // publicValues and proof are only used in test_ValidGroth16ProofUpdatesRoot.
        string memory json = vm.readFile(
            string.concat(
                vm.projectRoot(),
                "/src/fixtures/groth16-fixture.json"
            )
        );
        fixtureVkey = json.readBytes32(".vkey");
        fixturePublicValues = json.readBytes(".publicValues");
        fixtureProof = json.readBytes(".proof");

        zeko = new ZekoProofVerifier(
            address(gateway),
            fixtureVkey,
            VK_HASH,
            ACTION_STATE,
            CURRENT_ROOT
        );
    }

    function test_SetUp() public view {
        assertEq(address(zeko.verifier()), SP1_GATEWAY);
        assertEq(zeko.programVKey(), fixtureVkey);
        assertEq(zeko.owner(), owner);
        assertEq(zeko.pendingOwner(), address(0));
        assertEq(zeko.vkHash(), VK_HASH);
        assertEq(zeko.actionState(), ACTION_STATE);
        assertEq(zeko.currentRoot(), CURRENT_ROOT);
        assertTrue(zeko.validActionState(ACTION_STATE));
        assertTrue(zeko.isActionStateValid(ACTION_STATE));
    }

    function test_DecodePublicValues() public view {
        bytes32 newRoot = bytes32(uint256(0xdeadbeef));
        bytes memory pv = _buildPublicValues(
            true,
            VK_HASH,
            CURRENT_ROOT,
            newRoot,
            ACTION_STATE
        );

        ZekoProofVerifier.DecodedPublicValues memory d = zeko
            .getDecodedPublicValues(pv);

        assertTrue(d.proofValid);
        assertEq(d.vkHash, VK_HASH);
        assertEq(d.actionStateBefore, ACTION_STATE);
        assertEq(d.stateBefore[3], CURRENT_ROOT);
        assertEq(d.stateAfter[3], newRoot);
    }

    function test_RevertOnInvalidPublicValuesLength() public {
        bytes memory invalid = new bytes(16);
        vm.expectRevert(
            abi.encodeWithSelector(
                ZekoProofVerifier.InvalidPublicValuesLength.selector,
                PUBLIC_VALUES_LENGTH,
                invalid.length
            )
        );
        zeko.getDecodedPublicValues(invalid);
    }

    function test_RevertOnInvalidBool() public {
        bytes memory pv = _buildPublicValues(
            true,
            VK_HASH,
            CURRENT_ROOT,
            bytes32(0),
            ACTION_STATE
        );
        pv[0] = bytes1(uint8(2));
        vm.expectRevert(
            abi.encodeWithSelector(
                ZekoProofVerifier.InvalidBool.selector,
                uint8(2)
            )
        );
        zeko.getDecodedPublicValues(pv);
    }

    function test_ValidGroth16ProofUpdatesRoot() public {
        ZekoProofVerifier.DecodedPublicValues
            memory decoded = _decodePublicValues(fixturePublicValues);

        uint256 gasBefore = gasleft();
        zeko.verifyAndUpdateRoot(fixturePublicValues, fixtureProof);
        uint256 gasUsed = gasBefore - gasleft();

        console2.log("=== GROTH16 MAINNET FORK ===");
        console2.log("root before:");
        console2.logBytes32(decoded.stateBefore[3]);
        console2.log("root after:");
        console2.logBytes32(decoded.stateAfter[3]);
        console2.log("gas:", gasUsed);

        assertEq(zeko.currentRoot(), decoded.stateAfter[3]);
    }

    function test_RevertOnInvalidGroth16Proof() public {
        bytes memory fakeProof = new bytes(fixtureProof.length);
        vm.expectRevert();
        zeko.verifyAndUpdateRoot(fixturePublicValues, fakeProof);
    }

    function test_RevertOnInvalidVkHash() public {
        // Re-deploy with the fixture's actual vkHash so the SP1 proof passes,
        // then set a bad vkHash so ZekoProofVerifier rejects it.
        ZekoProofVerifier.DecodedPublicValues
            memory decoded = _decodePublicValues(fixturePublicValues);

        ZekoProofVerifier target = new ZekoProofVerifier(
            address(gateway),
            fixtureVkey,
            decoded.vkHash,
            decoded.actionStateBefore,
            decoded.stateBefore[3]
        );

        bytes32 badVkHash = keccak256("bad vk hash");
        target.setVkHash(badVkHash);

        vm.expectRevert(
            abi.encodeWithSelector(
                ZekoProofVerifier.InvalidVkHash.selector,
                badVkHash,
                decoded.vkHash
            )
        );
        target.verifyAndUpdateRoot(fixturePublicValues, fixtureProof);
    }

    function test_RevertOnInvalidActionState() public {
        ZekoProofVerifier.DecodedPublicValues
            memory decoded = _decodePublicValues(fixturePublicValues);

        ZekoProofVerifier target = new ZekoProofVerifier(
            address(gateway),
            fixtureVkey,
            decoded.vkHash,
            decoded.actionStateBefore,
            decoded.stateBefore[3]
        );

        bytes32 badActionState = keccak256("bad action state");
        target.setActionState(badActionState);

        vm.expectRevert(
            abi.encodeWithSelector(
                ZekoProofVerifier.InvalidActionState.selector,
                badActionState,
                decoded.actionStateBefore
            )
        );
        target.verifyAndUpdateRoot(fixturePublicValues, fixtureProof);
    }

    function test_RevertOnInvalidCurrentRoot() public {
        ZekoProofVerifier.DecodedPublicValues
            memory decoded = _decodePublicValues(fixturePublicValues);

        bytes32 badRoot = keccak256("bad root");

        ZekoProofVerifier target = new ZekoProofVerifier(
            address(gateway),
            fixtureVkey,
            decoded.vkHash,
            decoded.actionStateBefore,
            badRoot
        );

        vm.expectRevert(
            abi.encodeWithSelector(
                ZekoProofVerifier.InvalidCurrentRoot.selector,
                badRoot,
                decoded.stateBefore[3]
            )
        );
        target.verifyAndUpdateRoot(fixturePublicValues, fixtureProof);
    }

    function test_SetVkHashOnlyOwner() public {
        zeko.setVkHash(keccak256("new vk hash"));
        assertEq(zeko.vkHash(), keccak256("new vk hash"));
    }

    function test_RevertSetVkHashWhenNotOwner() public {
        vm.prank(alice);
        vm.expectRevert(ZekoProofVerifier.NotOwner.selector);
        zeko.setVkHash(keccak256("new vk hash"));
    }

    function test_SetActionStateOnlyOwner() public {
        bytes32 newActionState = keccak256("new action state");
        zeko.setActionState(newActionState);
        assertEq(zeko.actionState(), newActionState);
        assertTrue(zeko.validActionState(newActionState));
        assertTrue(zeko.isActionStateValid(newActionState));
    }

    function test_RevertSetActionStateWhenNotOwner() public {
        vm.prank(alice);
        vm.expectRevert(ZekoProofVerifier.NotOwner.selector);
        zeko.setActionState(keccak256("new action state"));
    }

    function test_TwoStepOwnershipTransfer() public {
        zeko.transferOwnership(alice);
        assertEq(zeko.owner(), owner);
        assertEq(zeko.pendingOwner(), alice);

        vm.prank(alice);
        zeko.acceptOwnership();
        assertEq(zeko.owner(), alice);
        assertEq(zeko.pendingOwner(), address(0));
    }

    function test_RevertTransferOwnershipWhenNotOwner() public {
        vm.prank(alice);
        vm.expectRevert(ZekoProofVerifier.NotOwner.selector);
        zeko.transferOwnership(bob);
    }

    function test_RevertTransferOwnershipToZeroAddress() public {
        vm.expectRevert(ZekoProofVerifier.ZeroAddress.selector);
        zeko.transferOwnership(address(0));
    }

    function test_RevertAcceptOwnershipWhenNotPendingOwner() public {
        zeko.transferOwnership(alice);
        vm.prank(bob);
        vm.expectRevert(ZekoProofVerifier.NotPendingOwner.selector);
        zeko.acceptOwnership();
    }

    function test_CancelOwnershipTransfer() public {
        zeko.transferOwnership(alice);
        zeko.cancelOwnershipTransfer();
        assertEq(zeko.owner(), owner);
        assertEq(zeko.pendingOwner(), address(0));
    }

    function test_RevertCancelOwnershipTransferWhenNotOwner() public {
        zeko.transferOwnership(alice);
        vm.prank(alice);
        vm.expectRevert(ZekoProofVerifier.NotOwner.selector);
        zeko.cancelOwnershipTransfer();
    }

    function _buildPublicValues(
        bool proofValid,
        bytes32 vkHash,
        bytes32 stateBefore3,
        bytes32 stateAfter3,
        bytes32 actionStateBefore
    ) internal pure returns (bytes memory pv) {
        pv = new bytes(PUBLIC_VALUES_LENGTH);
        uint256 cursor = 0;

        pv[cursor] = proofValid ? bytes1(uint8(1)) : bytes1(uint8(0));
        cursor += 1;

        _wb32(pv, cursor, vkHash);
        cursor += 32;

        for (uint256 i = 0; i < STATE_ARRAY_LENGTH; i++) {
            _wb32(pv, cursor, i == 3 ? stateBefore3 : bytes32(0));
            cursor += 32;
        }
        for (uint256 i = 0; i < STATE_ARRAY_LENGTH; i++) {
            _wb32(pv, cursor, i == 3 ? stateAfter3 : bytes32(0));
            cursor += 32;
        }

        _wb32(pv, cursor, actionStateBefore);
        cursor += 32;
        assert(cursor == PUBLIC_VALUES_LENGTH);
    }

    function _wb32(bytes memory buf, uint256 offset, bytes32 v) private pure {
        assembly {
            mstore(add(add(buf, 0x20), offset), v)
        }
    }

    function _decodePublicValues(
        bytes memory pv
    ) private pure returns (ZekoProofVerifier.DecodedPublicValues memory d) {
        require(pv.length == PUBLIC_VALUES_LENGTH, "invalid length");
        uint256 cursor = 0;

        uint8 raw = uint8(pv[cursor]);
        require(raw <= 1, "invalid bool");
        d.proofValid = raw == 1;
        cursor += 1;

        d.vkHash = _rb32(pv, cursor);
        cursor += 32;

        for (uint256 i = 0; i < STATE_ARRAY_LENGTH; i++) {
            d.stateBefore[i] = _rb32(pv, cursor);
            cursor += 32;
        }
        for (uint256 i = 0; i < STATE_ARRAY_LENGTH; i++) {
            d.stateAfter[i] = _rb32(pv, cursor);
            cursor += 32;
        }

        d.actionStateBefore = _rb32(pv, cursor);
        cursor += 32;
        assert(cursor == PUBLIC_VALUES_LENGTH);
    }

    function _rb32(
        bytes memory data,
        uint256 offset
    ) private pure returns (bytes32 v) {
        assembly {
            v := mload(add(add(data, 0x20), offset))
        }
    }
}
