// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {Pausable} from "@openzeppelin/contracts/utils/Pausable.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";

/// @title EthereumZekoBridge
/// @notice Ethereum-side bridge contract for Zeko.
/// @dev Withdrawals are intentionally not implemented yet; this version only handles deposits.
/// @dev Each deposit updates an append-only sequential state:
///      newDepositState = keccak256(DEPOSIT_STATE_DOMAIN, oldDepositState, depositLeaf)
contract EthereumZekoBridge is Ownable, Pausable, ReentrancyGuard {
    using SafeERC20 for IERC20;

    // -------------------------------------------------------------------------
    // Errors
    // -------------------------------------------------------------------------

    error ZeroAddress();
    error ZeroAmount();
    error EmptyZekoRecipient();
    error FeeOnTransferTokenNotSupported();
    error TokenNotAllowed(address token);
    error InvalidCheckpointNonce(uint64 nonce);

    // -------------------------------------------------------------------------
    // Constants
    // -------------------------------------------------------------------------

    bytes32 public constant INITIAL_DEPOSIT_STATE =
        keccak256("ZEKO_BRIDGE_INITIAL_DEPOSIT_STATE_V1");

    bytes32 public constant DEPOSIT_LEAF_DOMAIN =
        keccak256("ZEKO_BRIDGE_DEPOSIT_LEAF_V1");

    bytes32 public constant DEPOSIT_STATE_DOMAIN =
        keccak256("ZEKO_BRIDGE_DEPOSIT_STATE_V1");

    // -------------------------------------------------------------------------
    // Storage
    // -------------------------------------------------------------------------

    /// @notice Last deposit nonce. Starts at 0.
    uint64 public depositNonce;

    /// @notice Current Ethereum deposit accumulator state.
    bytes32 public currentDepositState;

    /// @notice Historical deposit state by nonce.
    /// @dev depositStateByNonce[0] is INITIAL_DEPOSIT_STATE.
    mapping(uint64 => bytes32) public depositStateByNonce;

    /// @notice Optional token allowlist.
    mapping(address => bool) public allowedToken;

    /// @notice Total deposited amount per token.
    mapping(address => uint256) public totalDepositedByToken;

    // -------------------------------------------------------------------------
    // Events
    // -------------------------------------------------------------------------

    event TokenAllowed(address indexed token, bool allowed);

    event BridgeDeposit(
        uint64 indexed nonce,
        bytes32 indexed depositLeaf,
        bytes32 indexed newDepositState,
        bytes32 oldDepositState,
        address token,
        address sender,
        bytes zekoRecipient,
        bytes32 zekoRecipientHash,
        uint256 amount
    );

    event EmergencyTokenWithdraw(
        address indexed token,
        address indexed to,
        uint256 amount
    );

    // -------------------------------------------------------------------------
    // Constructor
    // -------------------------------------------------------------------------

    constructor(address initialOwner) Ownable(initialOwner) {
        if (initialOwner == address(0)) revert ZeroAddress();

        currentDepositState = INITIAL_DEPOSIT_STATE;
        depositStateByNonce[0] = INITIAL_DEPOSIT_STATE;
    }

    // -------------------------------------------------------------------------
    // Admin
    // -------------------------------------------------------------------------

    function setAllowedToken(address token, bool allowed) external onlyOwner {
        if (token == address(0)) revert ZeroAddress();

        allowedToken[token] = allowed;

        emit TokenAllowed(token, allowed);
    }

    function pause() external onlyOwner {
        _pause();
    }

    function unpause() external onlyOwner {
        _unpause();
    }

    /// @notice Emergency withdrawal for stuck funds.
    /// @dev Use carefully. For a production bridge, prefer a timelock or governance flow.
    function emergencyWithdrawToken(
        address token,
        address to,
        uint256 amount
    ) external onlyOwner nonReentrant {
        if (token == address(0) || to == address(0)) revert ZeroAddress();
        if (amount == 0) revert ZeroAmount();

        IERC20(token).safeTransfer(to, amount);

        emit EmergencyTokenWithdraw(token, to, amount);
    }

    // -------------------------------------------------------------------------
    // Deposit
    // -------------------------------------------------------------------------

    /// @notice Deposits ERC20 tokens and appends a deposit leaf to the bridge accumulator.
    /// @param token ERC20 token address.
    /// @param amount Token amount to lock on Ethereum.
    /// @param zekoRecipient Encoded Zeko recipient. The exact format must be defined by your protocol.
    function deposit(
        address token,
        uint256 amount,
        bytes calldata zekoRecipient
    ) external nonReentrant whenNotPaused returns (uint64 nonce, bytes32 depositLeaf, bytes32 newDepositState) {
        if (token == address(0)) revert ZeroAddress();
        if (!allowedToken[token]) revert TokenNotAllowed(token);
        if (amount == 0) revert ZeroAmount();
        if (zekoRecipient.length == 0) revert EmptyZekoRecipient();

        // Transfer first so fee-on-transfer tokens can be rejected by balance delta.
        // For a strict bridge, the received amount must equal the requested amount.
        uint256 balanceBefore = IERC20(token).balanceOf(address(this));
        IERC20(token).safeTransferFrom(msg.sender, address(this), amount);
        uint256 balanceAfter = IERC20(token).balanceOf(address(this));

        uint256 receivedAmount = balanceAfter - balanceBefore;
        if (receivedAmount != amount) revert FeeOnTransferTokenNotSupported();

        nonce = depositNonce + 1;

        bytes32 oldDepositState = currentDepositState;
        bytes32 zekoRecipientHash = keccak256(zekoRecipient);

        depositLeaf = computeDepositLeaf({
            token: token,
            sender: msg.sender,
            zekoRecipientHash: zekoRecipientHash,
            amount: amount,
            nonce: nonce
        });

        newDepositState = computeNextDepositState(oldDepositState, depositLeaf);

        depositNonce = nonce;
        currentDepositState = newDepositState;
        depositStateByNonce[nonce] = newDepositState;
        totalDepositedByToken[token] += amount;

        emit BridgeDeposit({
            nonce: nonce,
            depositLeaf: depositLeaf,
            newDepositState: newDepositState,
            oldDepositState: oldDepositState,
            token: token,
            sender: msg.sender,
            zekoRecipient: zekoRecipient,
            zekoRecipientHash: zekoRecipientHash,
            amount: amount
        });
    }

    // -------------------------------------------------------------------------
    // View helpers
    // -------------------------------------------------------------------------

    /// @notice Returns whether a checkpoint exists for a nonce.
    /// @dev Nonce 0 always exists because it is the initial state.
    function hasDepositState(uint64 nonce) external view returns (bool) {
        if (nonce == 0) return depositStateByNonce[0] == INITIAL_DEPOSIT_STATE;
        return nonce <= depositNonce && depositStateByNonce[nonce] != bytes32(0);
    }

    /// @notice Returns a historical deposit state, reverting if the nonce does not exist yet.
    function getDepositStateAt(uint64 nonce) external view returns (bytes32) {
        if (nonce > depositNonce) revert InvalidCheckpointNonce(nonce);

        return depositStateByNonce[nonce];
    }

    /// @notice Computes the canonical deposit leaf used by the accumulator.
    function computeDepositLeaf(
        address token,
        address sender,
        bytes32 zekoRecipientHash,
        uint256 amount,
        uint64 nonce
    ) public view returns (bytes32) {
        return keccak256(
            abi.encode(
                DEPOSIT_LEAF_DOMAIN,
                block.chainid,
                address(this),
                token,
                sender,
                zekoRecipientHash,
                amount,
                nonce
            )
        );
    }

    /// @notice Computes the next accumulator state from an old state and a deposit leaf.
    function computeNextDepositState(
        bytes32 oldDepositState,
        bytes32 depositLeaf
    ) public pure returns (bytes32) {
        return keccak256(
            abi.encode(
                DEPOSIT_STATE_DOMAIN,
                oldDepositState,
                depositLeaf
            )
        );
    }
}
