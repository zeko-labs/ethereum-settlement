// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Script, console2} from "forge-std/Script.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

import {ZekoSettlement} from "../src/ZekoSettlement.sol";
import {LocalSP1Verifier} from "../src/mocks/LocalSP1Verifier.sol";

/// @notice Deploys a local settlement proxy initialized from an OCaml fixture.
contract DeployLocalSettlement is Script {
    function run()
        external
        returns (LocalSP1Verifier verifier, ZekoSettlement implementation, ZekoSettlement settlement)
    {
        uint256 privateKey = vm.envUint("PRIVATE_KEY");
        address admin = vm.addr(privateKey);
        bytes32[8] memory initialOuterState;
        for (uint256 i = 0; i < initialOuterState.length; i++) {
            initialOuterState[i] = vm.envBytes32(string.concat("INITIAL_OUTER_STATE_", vm.toString(i)));
        }

        vm.startBroadcast(privateKey);
        verifier = new LocalSP1Verifier();
        implementation = new ZekoSettlement();
        ERC1967Proxy proxy = new ERC1967Proxy(
            address(implementation),
            abi.encodeCall(
                ZekoSettlement.initialize,
                (
                    admin,
                    address(verifier),
                    vm.envBytes32("SETTLEMENT_PROGRAM_VKEY"),
                    vm.envBytes32("SETTLEMENT_VK_HASH"),
                    initialOuterState,
                    vm.envBytes32("INITIAL_OUTER_ACTION_STATE"),
                    uint32(0),
                    uint64(block.timestamp),
                    uint32(60),
                    uint32(0)
                )
            )
        );
        vm.stopBroadcast();

        settlement = ZekoSettlement(address(proxy));
        console2.log("LOCAL_SP1_VERIFIER", address(verifier));
        console2.log("SETTLEMENT_IMPLEMENTATION", address(implementation));
        console2.log("SETTLEMENT_CONTRACT_ADDRESS", address(settlement));
    }
}
