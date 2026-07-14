// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

/// @dev OpenZeppelin 5.6 rejects an empty initialization payload. This narrow
/// subclass permits it only so the owner-gated factory can initialize the
/// proxy atomically immediately after CREATE2 returns.
contract PocERC1967Proxy is ERC1967Proxy {
    constructor(address implementation) ERC1967Proxy(implementation, bytes("")) {}

    function _unsafeAllowUninitialized() internal pure override returns (bool) {
        return true;
    }
}

/// @notice Owner-gated CREATE2 factory used to give the PoC stable proxy
/// addresses before the address-bound OCaml bridge circuits are compiled.
/// @dev Proxies are deployed without constructor initialization data so their
/// address is independent of circuit/SP1 keys. Initialization is performed in
/// the same transaction, and the CREATE2 deployment rolls back if it fails.
contract PocDeterministicFactory {
    address public immutable owner;

    error NotOwner(address caller);
    error ZeroAddress();
    error DeploymentFailed(bytes32 salt);
    error AlreadyDeployed(address deployment);
    error InitializationFailed(bytes reason);

    event CodeDeployed(bytes32 indexed salt, address indexed deployment);
    event ProxyDeployed(bytes32 indexed salt, address indexed proxy, address indexed implementation);

    constructor(address owner_) {
        if (owner_ == address(0)) revert ZeroAddress();
        owner = owner_;
    }

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner(msg.sender);
        _;
    }

    function deployCode(bytes32 salt, bytes calldata creationCode) external onlyOwner returns (address deployment) {
        deployment = predictCodeAddress(salt, keccak256(creationCode));
        if (deployment.code.length != 0) revert AlreadyDeployed(deployment);
        bytes memory code = creationCode;
        assembly ("memory-safe") {
            deployment := create2(0, add(code, 0x20), mload(code), salt)
        }
        if (deployment == address(0)) revert DeploymentFailed(salt);
        emit CodeDeployed(salt, deployment);
    }

    function deployProxyAndCall(bytes32 salt, address implementation, bytes calldata initialization)
        external
        onlyOwner
        returns (address proxy)
    {
        if (implementation.code.length == 0) revert ZeroAddress();
        proxy = predictProxyAddress(salt, implementation);
        if (proxy.code.length != 0) revert AlreadyDeployed(proxy);

        bytes memory creationCode = abi.encodePacked(type(PocERC1967Proxy).creationCode, abi.encode(implementation));
        assembly ("memory-safe") {
            proxy := create2(0, add(creationCode, 0x20), mload(creationCode), salt)
        }
        if (proxy == address(0)) revert DeploymentFailed(salt);

        (bool success, bytes memory result) = proxy.call(initialization);
        if (!success) revert InitializationFailed(result);
        emit ProxyDeployed(salt, proxy, implementation);
    }

    function predictCodeAddress(bytes32 salt, bytes32 creationCodeHash) public view returns (address) {
        return
            address(uint160(uint256(keccak256(abi.encodePacked(bytes1(0xff), address(this), salt, creationCodeHash)))));
    }

    function predictProxyAddress(bytes32 salt, address implementation) public view returns (address) {
        bytes32 creationCodeHash =
            keccak256(abi.encodePacked(type(PocERC1967Proxy).creationCode, abi.encode(implementation)));
        return predictCodeAddress(salt, creationCodeHash);
    }
}
