// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {console2} from "forge-std/Script.sol";

import {PocDeploymentConfig} from "./PocDeploymentConfig.sol";
import {PocDeterministicFactory} from "../src/PocDeterministicFactory.sol";
import {ZekoSettlement} from "../src/ZekoSettlement.sol";
import {EthereumZekoBridge} from "../src/EthereumZekoBridge.sol";
import {
    AssetRecord,
    AssetStatus,
    IZekoAssetRegistry,
    ZekoAssetRegistry
} from "../src/ZekoAssetRegistry.sol";
import {LocalSP1Verifier} from "../src/mocks/LocalSP1Verifier.sol";
import {PocERC20} from "../src/mocks/PocERC20.sol";

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
        address upgrader = vm.envOr("UPGRADER_ADDRESS", admin);
        require(gatewayProver != address(0) && upgrader != address(0), "role address is zero");
        if (!localMockVerifier) {
            require(gatewayProver != admin, "gateway prover must be separate from admin");
            require(upgrader != admin, "upgrader must be separate from admin");
            require(upgrader != gatewayProver, "upgrader must be separate from prover");
        }
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
        if (predicted.assetRegistryModule.code.length == 0) {
            require(
                factory.deployCode(ASSET_REGISTRY_MODULE_SALT, type(ZekoAssetRegistry).creationCode)
                    == predicted.assetRegistryModule,
                "asset registry module drift"
            );
        }
        if (predicted.bridgeImplementation.code.length == 0) {
            bytes memory bridgeCreationCode =
                abi.encodePacked(type(EthereumZekoBridge).creationCode, abi.encode(predicted.assetRegistryModule));
            require(
                factory.deployCode(BRIDGE_IMPLEMENTATION_SALT, bridgeCreationCode)
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
        IZekoAssetRegistry assetRegistry = IZekoAssetRegistry(address(bridge));

        bool erc20TokenEnabled = vm.envOr("ERC20_TOKEN_ENABLED", false);
        if (erc20TokenEnabled) {
            address[2] memory predictedTokens = [predicted.erc20Token0, predicted.erc20Token1];
            bytes32[2] memory tokenSalts = [ERC20_TOKEN_0_SALT, ERC20_TOKEN_1_SALT];
            for (uint32 index = 0; index < 2; index++) {
                string memory prefix = string.concat("ERC20_TOKEN_", vm.toString(index), "_");
                if (predictedTokens[index].code.length == 0) {
                    bytes memory tokenCreationCode = abi.encodePacked(type(PocERC20).creationCode, abi.encode(admin));
                    require(
                        factory.deployCode(tokenSalts[index], tokenCreationCode) == predictedTokens[index],
                        "ERC20 token drift"
                    );
                }
                PocERC20 token = PocERC20(predictedTokens[index]);
                require(token.mintAuthority() == admin, "ERC20 mint authority drift");
                if (assetRegistry.assetStatusByToken(address(token)) == AssetStatus.None) {
                    assetRegistry.proposeAsset(
                        AssetRecord({
                            schemaVersion: 1,
                            registryIndex: index,
                            assetId: vm.envBytes32(string.concat(prefix, "ASSET_ID")),
                            ethereumToken: address(token),
                            tokenOwnerL2: vm.envBytes32(string.concat(prefix, "OWNER_PACKED")),
                            tokenIdL2: vm.envBytes32(string.concat(prefix, "TOKEN_ID")),
                            decimals: 9,
                            inventoryCap: uint64(vm.envUint(string.concat(prefix, "DEPOSIT_CAP"))),
                            mftStandardVkId: vm.envBytes32("ERC20_MFT_STANDARD_VK_ID"),
                            vaultPublicKey: vm.envBytes32("ERC20_SHARED_VAULT_PACKED"),
                            universalBridgeVkId: vm.envBytes32("ERC20_UNIVERSAL_BRIDGE_VK_ID")
                        })
                    );
                }
                uint256 depositAmount = vm.envUint(string.concat(prefix, "DEPOSIT_AMOUNT"));
                if (token.balanceOf(admin) == 0) {
                    token.mint(admin, depositAmount);
                }
                require(token.balanceOf(admin) == depositAmount, "ERC20 depositor balance drift");
            }
        }

        settlement.setBridgeContract(address(bridge));
        bridge.setWithdrawalDelaySlots(withdrawalDelay);
        if (gatewayProver != admin) {
            settlement.grantRole(settlement.PROVER_ROLE(), gatewayProver);
            bridge.grantRole(bridge.PROVER_ROLE(), gatewayProver);
            settlement.revokeRole(settlement.PROVER_ROLE(), admin);
            bridge.revokeRole(bridge.PROVER_ROLE(), admin);
        }
        if (upgrader != admin) {
            settlement.grantRole(settlement.UPGRADER_ROLE(), upgrader);
            bridge.grantRole(bridge.UPGRADER_ROLE(), upgrader);
            settlement.revokeRole(settlement.UPGRADER_ROLE(), admin);
            bridge.revokeRole(bridge.UPGRADER_ROLE(), admin);
        }
        vm.stopBroadcast();

        console2.log("POC_FACTORY_ADDRESS", address(factory));
        console2.log("SETTLEMENT_IMPLEMENTATION_ADDRESS", predicted.settlementImplementation);
        console2.log("ASSET_REGISTRY_MODULE_ADDRESS", predicted.assetRegistryModule);
        console2.log("BRIDGE_IMPLEMENTATION_ADDRESS", predicted.bridgeImplementation);
        console2.log("SETTLEMENT_CONTRACT_ADDRESS", address(settlement));
        console2.log("BRIDGE_CONTRACT_ADDRESS", address(bridge));
        console2.log("GATEWAY_PROVER_ADDRESS", gatewayProver);
        console2.log("UPGRADER_ADDRESS", upgrader);
        console2.log("WITHDRAWAL_DELAY_SLOTS", withdrawalDelay);
        if (erc20TokenEnabled) {
            console2.log("ERC20_TOKEN_0_ADDRESS", predicted.erc20Token0);
            console2.log("ERC20_TOKEN_1_ADDRESS", predicted.erc20Token1);
            console2.log("ERC20_TOKEN_0_STATUS", uint8(assetRegistry.assetStatusByToken(predicted.erc20Token0)));
            console2.log("ERC20_TOKEN_1_STATUS", uint8(assetRegistry.assetStatusByToken(predicted.erc20Token1)));
        }
    }
}
