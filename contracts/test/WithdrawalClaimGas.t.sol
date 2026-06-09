// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";

contract WithdrawalClaimGasHarness {
    bytes32 private constant WITHDRAW_STATE_DOMAIN =
        keccak256("ZEKO_BRIDGE_WITHDRAW_STATE_V1");
    bytes32 private constant WITHDRAW_MERKLE_NODE_DOMAIN =
        keccak256("ZEKO_BRIDGE_WITHDRAW_MERKLE_NODE_V1");
    bytes32 private constant WITHDRAW_NULLIFIER_DOMAIN =
        keccak256("ZEKO_BRIDGE_WITHDRAW_NULLIFIER_V1");

    mapping(bytes32 => bool) public validLegacyState;
    mapping(bytes32 => bytes32) public merkleRootByActionState;
    mapping(bytes32 => uint32) public withdrawCountByActionState;
    mapping(bytes32 => bool) public spent;

    receive() external payable {}

    function configureLegacy(bytes32 stateAfter) external {
        validLegacyState[stateAfter] = true;
    }

    function configureMerkle(
        bytes32 oldActionState,
        bytes32 root,
        uint32 withdrawCount
    ) external {
        merkleRootByActionState[oldActionState] = root;
        withdrawCountByActionState[oldActionState] = withdrawCount;
    }

    function claimLegacy(
        bytes32 stateBefore,
        bytes32 stateAfter,
        bytes32 withdrawLeaf,
        uint256 withdrawIndex,
        bytes32[] calldata leafHashes,
        address payable recipient
    ) external {
        require(validLegacyState[stateAfter]);
        require(withdrawIndex < leafHashes.length);

        bytes32 state = stateBefore;
        for (uint256 i = 0; i < leafHashes.length; i++) {
            bytes32 leaf = i == withdrawIndex
                ? withdrawLeaf
                : leafHashes[i];
            state = keccak256(
                abi.encode(WITHDRAW_STATE_DOMAIN, state, leaf)
            );
        }
        require(state == stateAfter);

        _spendAndTransfer(withdrawIndex, withdrawLeaf, recipient);
    }

    function claimMerkle(
        bytes32 oldActionState,
        bytes32 withdrawLeaf,
        uint256 withdrawIndex,
        bytes32[16] calldata proof,
        address payable recipient
    ) external {
        require(withdrawIndex < withdrawCountByActionState[oldActionState]);

        uint256 originalWithdrawIndex = withdrawIndex;
        bytes32 computed = withdrawLeaf;
        for (uint256 i = 0; i < 16; i++) {
            bytes32 sibling = proof[i];
            computed = (withdrawIndex & 1) == 0
                ? _hashNode(computed, sibling)
                : _hashNode(sibling, computed);
            withdrawIndex >>= 1;
        }
        require(computed == merkleRootByActionState[oldActionState]);

        _spendAndTransfer(originalWithdrawIndex, withdrawLeaf, recipient);
    }

    function _spendAndTransfer(
        uint256 withdrawIndex,
        bytes32 withdrawLeaf,
        address payable recipient
    ) private {
        bytes32 nullifier = keccak256(
            abi.encode(WITHDRAW_NULLIFIER_DOMAIN, withdrawIndex, withdrawLeaf)
        );
        require(!spent[nullifier]);
        spent[nullifier] = true;

        (bool success, ) = recipient.call{value: 1 wei}("");
        require(success);
    }

    function _hashNode(
        bytes32 left,
        bytes32 right
    ) private pure returns (bytes32) {
        return
            keccak256(
                abi.encode(WITHDRAW_MERKLE_NODE_DOMAIN, left, right)
            );
    }
}

contract WithdrawalClaimGasTest is Test {
    bytes32 private constant WITHDRAW_STATE_DOMAIN =
        keccak256("ZEKO_BRIDGE_WITHDRAW_STATE_V1");
    bytes32 private constant WITHDRAW_MERKLE_NODE_DOMAIN =
        keccak256("ZEKO_BRIDGE_WITHDRAW_MERKLE_NODE_V1");

    function test_GasClaimLegacy50HashesVersusMerkleProof() public {
        bytes32[] memory leaves = new bytes32[](50);
        for (uint256 i = 0; i < leaves.length; i++) {
            leaves[i] = keccak256(abi.encode("withdraw leaf", i));
        }

        bytes32 legacyState;
        for (uint256 i = 0; i < leaves.length; i++) {
            legacyState = keccak256(
                abi.encode(WITHDRAW_STATE_DOMAIN, legacyState, leaves[i])
            );
        }

        bytes32 root = _merkleRoot(leaves);
        bytes32[16] memory proof = _merkleProof(leaves, 25);
        bytes32 oldActionState = keccak256("old action state");

        WithdrawalClaimGasHarness legacy = new WithdrawalClaimGasHarness();
        WithdrawalClaimGasHarness merkle = new WithdrawalClaimGasHarness();
        vm.deal(address(legacy), 1 ether);
        vm.deal(address(merkle), 1 ether);
        legacy.configureLegacy(legacyState);
        merkle.configureMerkle(oldActionState, root, 50);

        uint256 gasBefore = gasleft();
        legacy.claimLegacy(
            bytes32(0),
            legacyState,
            leaves[25],
            25,
            leaves,
            payable(address(0xA11CE))
        );
        uint256 legacyExecutionGas = gasBefore - gasleft();

        gasBefore = gasleft();
        merkle.claimMerkle(
            oldActionState,
            leaves[25],
            25,
            proof,
            payable(address(0xB0B))
        );
        uint256 merkleExecutionGas = gasBefore - gasleft();

        bytes memory legacyCalldata = abi.encodeCall(
            legacy.claimLegacy,
            (
                bytes32(0),
                legacyState,
                leaves[25],
                25,
                leaves,
                payable(address(0xA11CE))
            )
        );
        bytes memory merkleCalldata = abi.encodeCall(
            merkle.claimMerkle,
            (
                oldActionState,
                leaves[25],
                25,
                proof,
                payable(address(0xB0B))
            )
        );
        uint256 legacyCalldataGas = _calldataGas(legacyCalldata);
        uint256 merkleCalldataGas = _calldataGas(merkleCalldata);

        emit log_named_uint("legacy_execution_gas", legacyExecutionGas);
        emit log_named_uint("merkle_execution_gas", merkleExecutionGas);
        emit log_named_uint("legacy_calldata_gas", legacyCalldataGas);
        emit log_named_uint("merkle_calldata_gas", merkleCalldataGas);
        emit log_named_uint(
            "legacy_total_gas",
            legacyExecutionGas + legacyCalldataGas
        );
        emit log_named_uint(
            "merkle_total_gas",
            merkleExecutionGas + merkleCalldataGas
        );
        emit log_named_uint(
            "total_gas_saved",
            legacyExecutionGas +
                legacyCalldataGas -
                merkleExecutionGas -
                merkleCalldataGas
        );
    }

    function _merkleRoot(
        bytes32[] memory leaves
    ) private pure returns (bytes32) {
        bytes32[17] memory zeroHashes = _zeroHashes();
        bytes32[] memory nodes = leaves;
        for (uint256 level = 0; level < 16; level++) {
            bytes32[] memory parents = new bytes32[]((nodes.length + 1) / 2);
            for (uint256 i = 0; i < nodes.length; i += 2) {
                bytes32 right = i + 1 < nodes.length
                    ? nodes[i + 1]
                    : zeroHashes[level];
                parents[i / 2] = _hashNode(nodes[i], right);
            }
            nodes = parents;
        }
        return nodes[0];
    }

    function _merkleProof(
        bytes32[] memory leaves,
        uint256 targetIndex
    ) private pure returns (bytes32[16] memory proof) {
        bytes32[17] memory zeroHashes = _zeroHashes();
        bytes32[] memory nodes = leaves;
        uint256 index = targetIndex;
        for (uint256 level = 0; level < 16; level++) {
            uint256 siblingIndex = index ^ 1;
            proof[level] = siblingIndex < nodes.length
                ? nodes[siblingIndex]
                : zeroHashes[level];

            bytes32[] memory parents = new bytes32[]((nodes.length + 1) / 2);
            for (uint256 i = 0; i < nodes.length; i += 2) {
                bytes32 right = i + 1 < nodes.length
                    ? nodes[i + 1]
                    : zeroHashes[level];
                parents[i / 2] = _hashNode(nodes[i], right);
            }
            nodes = parents;
            index >>= 1;
        }
    }

    function _zeroHashes() private pure returns (bytes32[17] memory hashes) {
        for (uint256 level = 0; level < 16; level++) {
            hashes[level + 1] = _hashNode(hashes[level], hashes[level]);
        }
    }

    function _hashNode(
        bytes32 left,
        bytes32 right
    ) private pure returns (bytes32) {
        return
            keccak256(
                abi.encode(WITHDRAW_MERKLE_NODE_DOMAIN, left, right)
            );
    }

    function _calldataGas(bytes memory data) private pure returns (uint256 gas) {
        for (uint256 i = 0; i < data.length; i++) {
            gas += data[i] == bytes1(0) ? 4 : 16;
        }
    }
}
