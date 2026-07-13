// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {ISP1Verifier} from "../ZekoSettlement.sol";

/// @notice Local execute-only verifier used by the gateway E2E harness.
/// @dev The execute-only worker never submits a proof to this contract. Keeping
///      a contract at the configured verifier address still exercises the same
///      settlement initialization and state-hydration path as a testnet deploy.
contract LocalSP1Verifier is ISP1Verifier {
    function verifyProof(bytes32, bytes calldata, bytes calldata) external pure {}
}
