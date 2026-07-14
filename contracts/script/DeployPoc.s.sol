// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {console2} from "forge-std/Script.sol";

import {PocDeploymentConfig} from "./PocDeploymentConfig.sol";
import {PocDeterministicFactory} from "../src/PocDeterministicFactory.sol";
import {ZekoSettlement} from "../src/ZekoSettlement.sol";
import {EthereumZekoBridge} from "../src/EthereumZekoBridge.sol";
import {LocalSP1Verifier} from "../src/mocks/LocalSP1Verifier.sol";

contract DeployPoc is PocDeploymentConfig {
    function run()
        external
        returns (PocDeterministicFactory factory, ZekoSettlement settlement, EthereumZekoBridge bridge)
    {
        uint256 privateKey = vm.envUint("PRIVATE_KEY");
        address broadcaster = vm.addr(privateKey);
        address admin = vm.envAddress("ADMIN_ADDRESS");
        require(broadcaster == admin, "PRIVATE_KEY must belong to ADMIN_ADDRESS");

        Addresses memory predicted = _predict(admin);
        bytes32[8] memory initialOuterState;
        for (uint256 i = 0; i < initialOuterState.length; i++) {
            initialOuterState[i] = vm.envBytes32(string.concat("INITIAL_OUTER_STATE_", vm.toString(i)));
        }
        bool localMockVerifier = vm.envOr("LOCAL_MOCK_VERIFIER", false);
        address verifier = localMockVerifier ? predicted.localVerifier : vm.envAddress("SP1_VERIFIER_ADDRESS");
        address gatewayProver = vm.envOr("GATEWAY_PROVER_ADDRESS", admin);
        uint256 defaultGenesisTimestamp = localMockVerifier ? block.timestamp + 1 days : block.timestamp;
        uint64 genesisTimestamp = uint64(vm.envOr("GENESIS_TIMESTAMP", defaultGenesisTimestamp));
        uint32 slotDuration = uint32(vm.envOr("SLOT_DURATION", uint256(12)));
        uint32 forkSlot = uint32(vm.envOr("FORK_SLOT", uint256(0)));
        uint32 withdrawalDelay = uint32(vm.envOr("WITHDRAWAL_DELAY_SLOTS", uint256(5)));

        vm.startBroadcast(privateKey);
        if (predicted.factory.code.length == 0) {
            factory = new PocDeterministicFactory{salt: FACTORY_SALT}(admin);
        } else {
            factory = PocDeterministicFactory(predicted.factory);
        }
        require(address(factory) == predicted.factory, "factory address drift");
        require(factory.owner() == admin, "factory owner drift");

        if (localMockVerifier && predicted.localVerifier.code.length == 0) {
            require(
                factory.deployCode(LOCAL_VERIFIER_SALT, type(LocalSP1Verifier).creationCode) == predicted.localVerifier,
                "local verifier drift"
            );
        }

        if (predicted.settlementImplementation.code.length == 0) {
            require(
                factory.deployCode(SETTLEMENT_IMPLEMENTATION_SALT, type(ZekoSettlement).creationCode)
                    == predicted.settlementImplementation,
                "settlement implementation drift"
            );
        }
        if (predicted.bridgeImplementation.code.length == 0) {
            require(
                factory.deployCode(BRIDGE_IMPLEMENTATION_SALT, type(EthereumZekoBridge).creationCode)
                    == predicted.bridgeImplementation,
                "bridge implementation drift"
            );
        }

        if (predicted.settlementProxy.code.length == 0) {
            factory.deployProxyAndCall(
                SETTLEMENT_PROXY_SALT,
                predicted.settlementImplementation,
                abi.encodeCall(
                    ZekoSettlement.initialize,
                    (
                        admin,
                        verifier,
                        vm.envBytes32("SETTLEMENT_PROGRAM_VKEY"),
                        vm.envBytes32("SETTLEMENT_VK_HASH"),
                        initialOuterState,
                        vm.envBytes32("INITIAL_OUTER_ACTION_STATE"),
                        uint32(0),
                        genesisTimestamp,
                        slotDuration,
                        forkSlot
                    )
                )
            );
        }
        settlement = ZekoSettlement(predicted.settlementProxy);

        if (predicted.bridgeProxy.code.length == 0) {
            factory.deployProxyAndCall(
                BRIDGE_PROXY_SALT,
                predicted.bridgeImplementation,
                abi.encodeCall(
                    EthereumZekoBridge.initialize,
                    (
                        admin,
                        address(settlement),
                        verifier,
                        vm.envBytes32("BRIDGE_PROGRAM_VKEY"),
                        verifier,
                        vm.envBytes32("WITHDRAW_PROGRAM_VKEY")
                    )
                )
            );
        }
        bridge = EthereumZekoBridge(payable(predicted.bridgeProxy));

        settlement.setBridgeContract(address(bridge));
        bridge.setWithdrawalDelaySlots(withdrawalDelay);
        if (gatewayProver != admin) {
            settlement.grantRole(settlement.PROVER_ROLE(), gatewayProver);
            bridge.grantRole(bridge.PROVER_ROLE(), gatewayProver);
        }
        vm.stopBroadcast();

        console2.log("POC_FACTORY_ADDRESS", address(factory));
        console2.log("SETTLEMENT_IMPLEMENTATION_ADDRESS", predicted.settlementImplementation);
        console2.log("BRIDGE_IMPLEMENTATION_ADDRESS", predicted.bridgeImplementation);
        console2.log("SETTLEMENT_CONTRACT_ADDRESS", address(settlement));
        console2.log("BRIDGE_CONTRACT_ADDRESS", address(bridge));
        console2.log("WITHDRAWAL_DELAY_SLOTS", withdrawalDelay);
    }
}
