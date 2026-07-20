// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";

import {PocDeterministicFactory} from "../src/PocDeterministicFactory.sol";
import {ZekoSettlement} from "../src/ZekoSettlement.sol";
import {LocalSP1Verifier} from "../src/mocks/LocalSP1Verifier.sol";

contract PocDeterministicFactoryTest is Test {
    bytes32 private constant IMPLEMENTATION_SALT = keccak256("settlement implementation");
    bytes32 private constant PROXY_SALT = keccak256("settlement proxy");

    PocDeterministicFactory private factory;
    LocalSP1Verifier private verifier;
    bytes32[8] private initialState;

    function setUp() public {
        factory = new PocDeterministicFactory(address(this));
        verifier = new LocalSP1Verifier();
        initialState[2] = keccak256("ledger");
        initialState[3] = keccak256("inner actions");
    }

    function test_PredictedAddressIsIndependentOfChainIdAndInitialization() public {
        address implementation =
            factory.predictCodeAddress(IMPLEMENTATION_SALT, keccak256(type(ZekoSettlement).creationCode));
        address predicted = factory.predictProxyAddress(PROXY_SALT, implementation);

        vm.chainId(11155111);
        assertEq(factory.predictProxyAddress(PROXY_SALT, implementation), predicted);

        implementation = factory.deployCode(IMPLEMENTATION_SALT, type(ZekoSettlement).creationCode);
        bytes memory initialization = _initialization(address(verifier));
        address proxy = factory.deployProxyAndCall(PROXY_SALT, implementation, initialization);
        assertEq(proxy, predicted);
        assertTrue(ZekoSettlement(proxy).hasRole(0x00, address(this)));

        vm.expectRevert(abi.encodeWithSelector(PocDeterministicFactory.AlreadyDeployed.selector, proxy));
        factory.deployProxyAndCall(PROXY_SALT, implementation, abi.encodePacked(bytes4(0xdeadbeef)));
    }

    function test_FailedInitializationRollsBackCreate2Deployment() public {
        address implementation = factory.deployCode(IMPLEMENTATION_SALT, type(ZekoSettlement).creationCode);
        address predicted = factory.predictProxyAddress(PROXY_SALT, implementation);

        vm.expectRevert(
            abi.encodeWithSelector(
                PocDeterministicFactory.InitializationFailed.selector,
                abi.encodeWithSelector(ZekoSettlement.ZeroAddress.selector)
            )
        );
        factory.deployProxyAndCall(PROXY_SALT, implementation, _initialization(address(0)));
        assertEq(predicted.code.length, 0);

        address proxy = factory.deployProxyAndCall(PROXY_SALT, implementation, _initialization(address(verifier)));
        assertEq(proxy, predicted);
        assertGt(proxy.code.length, 0);
    }

    function test_OnlyOwnerCanDeploy() public {
        vm.prank(address(0xB0B));
        vm.expectRevert(abi.encodeWithSelector(PocDeterministicFactory.NotOwner.selector, address(0xB0B)));
        factory.deployCode(IMPLEMENTATION_SALT, type(ZekoSettlement).creationCode);
    }

    function _initialization(address verifierAddress) private view returns (bytes memory) {
        return abi.encodeCall(
            ZekoSettlement.initialize,
            (
                address(this),
                verifierAddress,
                keccak256("program"),
                keccak256("vk"),
                initialState,
                keccak256("outer actions"),
                uint32(0),
                uint64(block.timestamp),
                uint32(12),
                uint32(0)
            )
        );
    }
}
