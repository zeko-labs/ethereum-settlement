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

        vm.serializeUint(object, "schemaVersion", 3);
        vm.serializeUint(object, "chainId", block.chainid);
        vm.serializeString(object, "dataAvailability", "multisig");
        vm.serializeString(object, "minaSigningNetworkId", vm.envOr("MINA_SIGNING_NETWORK_ID", string("testnet")));
        vm.serializeAddress(object, "admin", admin);
        vm.serializeAddress(object, "upgrader", vm.envOr("UPGRADER_ADDRESS", admin));
        vm.serializeAddress(object, "gatewayProver", vm.envOr("GATEWAY_PROVER_ADDRESS", admin));
        vm.serializeAddress(object, "factory", addresses.factory);
        vm.serializeAddress(object, "settlementImplementation", addresses.settlementImplementation);
        vm.serializeAddress(object, "bridgeImplementation", addresses.bridgeImplementation);
        vm.serializeAddress(object, "localSp1Verifier", addresses.localVerifier);
        vm.serializeAddress(object, "sp1Verifier", vm.envOr("SP1_VERIFIER_ADDRESS", addresses.localVerifier));
        vm.serializeAddress(object, "settlement", addresses.settlementProxy);
        vm.serializeAddress(object, "bridge", addresses.bridgeProxy);
        vm.serializeAddress(object, "assetRegistryContract", addresses.bridgeProxy);
        vm.serializeAddress(object, "erc20Token0", addresses.erc20Token0);
        vm.serializeAddress(object, "erc20Token1", addresses.erc20Token1);
        vm.serializeString(object, "assetRegistryZkapp", vm.envOr("ERC20_REGISTRY_L2", string("")));
        vm.serializeString(object, "sharedVaultL2", vm.envOr("ERC20_SHARED_VAULT_L2", string("")));
        vm.serializeBytes32(object, "mftStandardVkId", vm.envOr("ERC20_MFT_STANDARD_VK_ID", bytes32(0)));
        vm.serializeBytes32(
            object, "universalBridgeVkId", vm.envOr("ERC20_UNIVERSAL_BRIDGE_VK_ID", bytes32(0))
        );
        vm.serializeUint(object, "assetRegistrySchemaVersion", vm.envOr("ERC20_REGISTRY_SCHEMA_VERSION", uint256(1)));
        vm.serializeString(object, "zekoRevision", vm.envOr("ZEKO_SOURCE_REVISION", string("")));
        vm.serializeString(object, "settlementRevision", vm.envOr("SETTLEMENT_SOURCE_REVISION", string("")));
        vm.serializeString(object, "zekoUiRevision", vm.envOr("ZEKO_UI_SOURCE_REVISION", string("")));
        vm.serializeBytes32(object, "ocamlEthereumHolderX", bytes32(uint256(uint160(addresses.bridgeProxy))));
        vm.serializeBytes32(object, "settlementProgramVkey", vm.envBytes32("SETTLEMENT_PROGRAM_VKEY"));
        vm.serializeBytes32(object, "bridgeProgramVkey", vm.envBytes32("BRIDGE_PROGRAM_VKEY"));
        vm.serializeBytes32(object, "withdrawProgramVkey", vm.envBytes32("WITHDRAW_PROGRAM_VKEY"));
        manifest = vm.serializeBytes32(object, "settlementVkHash", vm.envBytes32("SETTLEMENT_VK_HASH"));
        vm.writeJson(manifest, vm.envString("POC_MANIFEST_PATH"));
    }
}
