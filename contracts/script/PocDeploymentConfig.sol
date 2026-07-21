// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Script} from "forge-std/Script.sol";

import {PocDeterministicFactory, PocERC1967Proxy} from "../src/PocDeterministicFactory.sol";
import {ZekoSettlement} from "../src/ZekoSettlement.sol";
import {EthereumZekoBridge} from "../src/EthereumZekoBridge.sol";
import {LocalSP1Verifier} from "../src/mocks/LocalSP1Verifier.sol";
import {PocERC20} from "../src/mocks/PocERC20.sol";

abstract contract PocDeploymentConfig is Script {
    bytes32 internal constant FACTORY_SALT = keccak256("zeko-poc-factory-v1");
    bytes32 internal constant SETTLEMENT_IMPLEMENTATION_SALT = keccak256("zeko-settlement-implementation-v1");
    bytes32 internal constant BRIDGE_IMPLEMENTATION_SALT = keccak256("zeko-bridge-implementation-v1");
    bytes32 internal constant SETTLEMENT_PROXY_SALT = keccak256("zeko-settlement-proxy-v1");
    bytes32 internal constant BRIDGE_PROXY_SALT = keccak256("zeko-bridge-proxy-v1");
    bytes32 internal constant LOCAL_VERIFIER_SALT = keccak256("zeko-local-sp1-verifier-v1");
    bytes32 internal constant ERC20_TOKEN_SALT = keccak256("zeko-poc-erc20-v1");

    struct Addresses {
        address factory;
        address settlementImplementation;
        address bridgeImplementation;
        address localVerifier;
        address settlementProxy;
        address bridgeProxy;
        address erc20Token;
    }

    function _predict(address admin) internal view returns (Addresses memory a) {
        bytes memory factoryCreationCode =
            abi.encodePacked(type(PocDeterministicFactory).creationCode, abi.encode(admin));
        a.factory = vm.computeCreate2Address(FACTORY_SALT, keccak256(factoryCreationCode));
        a.settlementImplementation =
            _create2Address(a.factory, SETTLEMENT_IMPLEMENTATION_SALT, keccak256(type(ZekoSettlement).creationCode));
        a.bridgeImplementation =
            _create2Address(a.factory, BRIDGE_IMPLEMENTATION_SALT, keccak256(type(EthereumZekoBridge).creationCode));
        a.localVerifier =
            _create2Address(a.factory, LOCAL_VERIFIER_SALT, keccak256(type(LocalSP1Verifier).creationCode));
        a.settlementProxy = _proxyAddress(a.factory, SETTLEMENT_PROXY_SALT, a.settlementImplementation);
        a.bridgeProxy = _proxyAddress(a.factory, BRIDGE_PROXY_SALT, a.bridgeImplementation);
        a.erc20Token = _create2Address(
            a.factory, ERC20_TOKEN_SALT, keccak256(abi.encodePacked(type(PocERC20).creationCode, abi.encode(admin)))
        );
    }

    function _proxyAddress(address factory, bytes32 salt, address implementation) private pure returns (address) {
        bytes32 hash = keccak256(abi.encodePacked(type(PocERC1967Proxy).creationCode, abi.encode(implementation)));
        return _create2Address(factory, salt, hash);
    }

    function _create2Address(address deployer, bytes32 salt, bytes32 creationCodeHash) private pure returns (address) {
        return address(uint160(uint256(keccak256(abi.encodePacked(bytes1(0xff), deployer, salt, creationCodeHash)))));
    }
}
