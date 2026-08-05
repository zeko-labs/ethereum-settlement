// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

/// @notice Deterministic nine-decimal ERC20 used by the local bridge roundtrip.
/// @dev The immutable externally-owned mint authority keeps the CREATE2 address
/// stable while preventing arbitrary test-process callers from inflating supply.
contract PocERC20 is ERC20 {
    error NotMintAuthority(address caller);
    error ZeroMintAuthority();

    address public immutable mintAuthority;

    constructor(address mintAuthority_) ERC20("Zeko PoC ERC20", "ZP20") {
        if (mintAuthority_ == address(0)) revert ZeroMintAuthority();
        mintAuthority = mintAuthority_;
    }

    function decimals() public pure override returns (uint8) {
        return 9;
    }

    function mint(address recipient, uint256 amount) external {
        if (msg.sender != mintAuthority) {
            revert NotMintAuthority(msg.sender);
        }
        _mint(recipient, amount);
    }
}
