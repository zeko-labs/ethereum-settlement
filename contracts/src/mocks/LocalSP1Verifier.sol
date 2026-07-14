// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {ISP1Verifier} from "../ZekoSettlement.sol";

/// @notice Local verifier used only after the gateway has executed the guest.
/// @dev This deliberately accepts an empty proof so the local-submit harness can
///      exercise contract state transitions without generating an SP1 proof.
contract LocalSP1Verifier is ISP1Verifier {
    function isLocalSP1Verifier() external pure returns (bool) {
        return true;
    }

    function verifyProof(bytes32, bytes calldata, bytes calldata) external pure {}
}
