// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {console2} from "forge-std/Script.sol";

import {PocDeploymentConfig} from "./PocDeploymentConfig.sol";

contract PredictPocDeployment is PocDeploymentConfig {
    function run() external view returns (Addresses memory addresses) {
        address admin = vm.envAddress("ADMIN_ADDRESS");
        addresses = _predict(admin);

        console2.log("ADMIN_ADDRESS", admin);
        console2.log("POC_FACTORY_ADDRESS", addresses.factory);
        console2.log("SETTLEMENT_IMPLEMENTATION_ADDRESS", addresses.settlementImplementation);
        console2.log("BRIDGE_IMPLEMENTATION_ADDRESS", addresses.bridgeImplementation);
        console2.log("LOCAL_SP1_VERIFIER_ADDRESS", addresses.localVerifier);
        console2.log("SETTLEMENT_CONTRACT_ADDRESS", addresses.settlementProxy);
        console2.log("BRIDGE_CONTRACT_ADDRESS", addresses.bridgeProxy);
        console2.logBytes32(bytes32(uint256(uint160(addresses.bridgeProxy))));
    }
}
