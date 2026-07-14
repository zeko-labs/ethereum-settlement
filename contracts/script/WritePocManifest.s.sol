// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {PocDeploymentConfig} from "./PocDeploymentConfig.sol";

/// @notice Writes the proof/build/deployment identity consumed by the local
/// stack. The manifest can be generated before deployment because every
/// contract address is deterministic.
contract WritePocManifest is PocDeploymentConfig {
    function run() external returns (string memory manifest) {
        address admin = vm.envAddress("ADMIN_ADDRESS");
        Addresses memory addresses = _predict(admin);
        string memory object = "poc";

        vm.serializeUint(object, "schemaVersion", 1);
        vm.serializeUint(object, "chainId", block.chainid);
        vm.serializeString(object, "dataAvailability", "multisig");
        vm.serializeAddress(object, "admin", admin);
        vm.serializeAddress(object, "factory", addresses.factory);
        vm.serializeAddress(object, "settlementImplementation", addresses.settlementImplementation);
        vm.serializeAddress(object, "bridgeImplementation", addresses.bridgeImplementation);
        vm.serializeAddress(object, "localSp1Verifier", addresses.localVerifier);
        vm.serializeAddress(object, "settlement", addresses.settlementProxy);
        vm.serializeAddress(object, "bridge", addresses.bridgeProxy);
        vm.serializeBytes32(object, "ocamlEthereumHolderX", bytes32(uint256(uint160(addresses.bridgeProxy))));
        vm.serializeBytes32(object, "settlementProgramVkey", vm.envBytes32("SETTLEMENT_PROGRAM_VKEY"));
        vm.serializeBytes32(object, "bridgeProgramVkey", vm.envBytes32("BRIDGE_PROGRAM_VKEY"));
        vm.serializeBytes32(object, "withdrawProgramVkey", vm.envBytes32("WITHDRAW_PROGRAM_VKEY"));
        manifest = vm.serializeBytes32(object, "settlementVkHash", vm.envBytes32("SETTLEMENT_VK_HASH"));
        vm.writeJson(manifest, vm.envString("POC_MANIFEST_PATH"));
    }
}
