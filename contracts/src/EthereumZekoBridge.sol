// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {
    IERC20Metadata
} from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Metadata.sol";
import {
    SafeERC20
} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {AccessControl} from "@openzeppelin/contracts/access/AccessControl.sol";
import {
    Initializable
} from "@openzeppelin/contracts/proxy/utils/Initializable.sol";
import {
    UUPSUpgradeable
} from "@openzeppelin/contracts/proxy/utils/UUPSUpgradeable.sol";
import {Pausable} from "@openzeppelin/contracts/utils/Pausable.sol";
import {
    ReentrancyGuard
} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {ZekoAddress, ZekoAddressLib} from "./ZekoAddress.sol";
import {ISP1Verifier} from "./ZekoSettlement.sol";

interface IZekoSettlementVerifier {
    function isActionStateValid(
        bytes32 actionState
    ) external view returns (bool);

    function l2ActionStateInfo(
        bytes32 actionState
    ) external view returns (uint64 index, bool valid);
}

/// @title EthereumZekoBridge
/// @notice Ethereum-side bridge contract for Zeko.
/// @dev Each deposit updates an append-only sequential state:
///      newDepositState = keccak256(DEPOSIT_STATE_DOMAIN, oldDepositState, depositLeaf)
contract EthereumZekoBridge is
    Initializable,
    AccessControl,
    UUPSUpgradeable,
    Pausable,
    ReentrancyGuard
{
    using SafeERC20 for IERC20;
    using ZekoAddressLib for ZekoAddress;

    // -------------------------------------------------------------------------
    // Errors
    // -------------------------------------------------------------------------

    error ZeroAddress();
    error ZeroAmount();
    error FeeOnTransferTokenNotSupported();
    error TokenNotAllowed(address token);
    error InvalidCheckpointNonce(uint64 nonce);
    error InvalidZekoDecimals(uint8 decimals);
    error InvalidEthereumDecimals(address token, uint8 expected, uint8 actual);
    error InvalidNativeEthereumDecimals(uint8 decimals);
    error InvalidAmountPrecision(
        address token,
        uint256 amount,
        uint8 ethereumDecimals,
        uint8 zekoDecimals
    );
    error NativeTransferFailed();
    error TokenAlreadyAdded(address token);
    error TokenNotAdded(address token);
    error InvalidSettlementActionState(bytes32 actionState);
    error InvalidL2ActionStateTransition(
        bytes32 oldActionState,
        bytes32 newActionState
    );
    error ActionStateAlreadyProcessed(bytes32 actionState);
    error InvalidBridgePublicValuesLength(uint256 expected, uint256 actual);
    error InvalidDepositState(bytes32 expected, bytes32 actual);
    error InvalidDepositNonce(uint64 expected, uint64 actual);
    error InvalidWithdrawState(bytes32 withdrawState);
    error InvalidWithdrawProof();
    error InvalidWithdrawToken(bytes32 token);
    error InvalidWithdrawRecipient(bytes32 recipient);
    error WithdrawAlreadyClaimed(bytes32 nullifier);

    // -------------------------------------------------------------------------
    // Constants
    // -------------------------------------------------------------------------

    bytes32 public constant INITIAL_DEPOSIT_STATE =
        keccak256("ZEKO_BRIDGE_INITIAL_DEPOSIT_STATE_V1");

    bytes32 public constant DEPOSIT_LEAF_DOMAIN =
        keccak256("ZEKO_BRIDGE_DEPOSIT_LEAF_V1");

    bytes32 public constant DEPOSIT_STATE_DOMAIN =
        keccak256("ZEKO_BRIDGE_DEPOSIT_STATE_V1");

    bytes32 public constant WITHDRAW_LEAF_DOMAIN =
        keccak256("ZEKO_BRIDGE_WITHDRAW_LEAF_V1");

    bytes32 public constant WITHDRAW_STATE_DOMAIN =
        keccak256("ZEKO_BRIDGE_WITHDRAW_STATE_V1");

    bytes32 public constant WITHDRAW_NULLIFIER_DOMAIN =
        keccak256("ZEKO_BRIDGE_WITHDRAW_NULLIFIER_V1");

    bytes32 public constant WITHDRAW_MERKLE_NODE_DOMAIN =
        keccak256("ZEKO_BRIDGE_WITHDRAW_MERKLE_NODE_V1");

    uint256 public constant WITHDRAW_MERKLE_TREE_DEPTH = 16;
    uint256 public constant MAX_WITHDRAW_COUNT =
        1 << WITHDRAW_MERKLE_TREE_DEPTH;

    uint256 private constant BRIDGE_PUBLIC_VALUES_LENGTH = 148;
    uint256 private constant WITHDRAW_PUBLIC_VALUES_LENGTH = 164;

    uint8 public constant MAX_ZEKO_DECIMALS = 9;
    uint8 public constant NATIVE_ETHEREUM_DECIMALS = 18;

    bytes32 public constant ADMIN_ROLE = keccak256("ADMIN_ROLE");
    bytes32 public constant PROVER_ROLE = keccak256("PROVER_ROLE");
    bytes32 public constant UPGRADER_ROLE = keccak256("UPGRADER_ROLE");

    struct TokenConfig {
        uint8 zekoDecimals;
        uint8 ethereumDecimals;
        bool allowed;
    }

    struct WithdrawClaim {
        /// @notice Token as a Zeko field. It must encode an Ethereum address in the low 160 bits.
        bytes32 token;
        /// @notice Recipient as a Zeko field. It must encode an Ethereum address in the low 160 bits.
        bytes32 recipient;
        /// @notice Amount as a Zeko field. Converted back to Ethereum decimals before transfer.
        bytes32 amount;
    }

    struct DecodedBridgePublicValues {
        bytes32 ethereumStateBefore;
        bytes32 ethereumStateAfter;
        uint64 ethereumNonceBefore;
        uint64 ethereumNonceAfter;
        bytes32 zekoActionStateBefore;
        bytes32 zekoActionStateAfter;
        uint32 depositCount;
    }

    struct DecodedWithdrawPublicValues {
        bytes32 zekoActionStateBefore;
        bytes32 zekoActionStateAfter;
        bytes32 ethereumWithdrawStateBefore;
        bytes32 ethereumWithdrawStateAfter;
        bytes32 withdrawalRoot;
        uint32 withdrawCount;
    }

    struct WithdrawalRootInfo {
        bytes32 withdrawalRoot;
        bytes32 withdrawStateBefore;
        bytes32 withdrawStateAfter;
        uint64 oldActionStateIndex;
        uint32 withdrawCount;
        bool valid;
    }

    // -------------------------------------------------------------------------
    // Storage
    // -------------------------------------------------------------------------

    /// @notice Last deposit nonce. Starts at 0.
    uint64 public depositNonce;

    /// @notice Current Ethereum deposit accumulator state.
    bytes32 public currentDepositState;

    /// @notice Current Ethereum withdraw accumulator state.
    bytes32 public currentWithdrawState;

    /// @notice L2 action-state index matched by the current withdraw accumulator.
    uint64 public currentWithdrawActionStateIndex;

    /// @notice Historical deposit state by nonce.
    /// @dev depositStateByNonce[0] is INITIAL_DEPOSIT_STATE.
    mapping(uint64 => bytes32) public depositStateByNonce;

    /// @notice Settlement action states already consumed by bridge transitions.
    mapping(bytes32 => bool) public processedActionState;

    /// @dev Deprecated storage retained for UUPS layout compatibility.
    mapping(bytes32 => bool) public validWithdrawState;

    /// @dev Deprecated storage retained for UUPS layout compatibility.
    mapping(bytes32 => bytes32) public withdrawStateOldActionState;
    /// @dev Deprecated storage retained for UUPS layout compatibility.
    mapping(bytes32 => uint64) public withdrawStateOldActionStateIndex;

    /// @notice Claimed withdraw nullifiers.
    mapping(bytes32 => bool) public spentWithdraw;

    /// @notice Token configuration by L1 token address. `address(0)` is native ETH.
    mapping(address => TokenConfig) public allowedToken;

    /// @notice Total deposited amount per token.
    mapping(address => uint256) public totalDepositedByToken;

    IZekoSettlementVerifier public settlementVerifier;
    ISP1Verifier public bridgeVerifier;
    bytes32 public bridgeProgramVKey;
    ISP1Verifier public withdrawVerifier;
    bytes32 public withdrawProgramVKey;

    /// @notice Accepted withdrawal batch information by old action state.
    mapping(bytes32 => WithdrawalRootInfo) public withdrawalRootInfo;

    // -------------------------------------------------------------------------
    // Events
    // -------------------------------------------------------------------------

    event TokenAllowed(
        address indexed token,
        bool allowed,
        uint8 zekoDecimals,
        uint8 ethereumDecimals
    );

    event BridgeDeposit(
        uint64 indexed nonce,
        bytes32 indexed depositLeaf,
        bytes32 indexed newDepositState,
        bytes32 oldDepositState,
        address token,
        address sender,
        ZekoAddress zekoRecipient,
        uint256 amount,
        uint256 zekoAmount,
        uint64 timeout
    );

    event EmergencyTokenWithdraw(
        address indexed token,
        address indexed to,
        uint256 amount
    );

    event WithdrawStateAccepted(
        bytes32 indexed oldActionState,
        bytes32 indexed actionState,
        bytes32 indexed oldWithdrawState,
        bytes32 newWithdrawState
    );

    event WithdrawalRootAccepted(
        bytes32 indexed oldActionState,
        bytes32 indexed newActionState,
        bytes32 indexed withdrawalRoot,
        bytes32 oldWithdrawState,
        bytes32 newWithdrawState,
        uint32 withdrawCount
    );

    event BridgeTransitionAccepted(
        bytes32 indexed oldActionState,
        bytes32 indexed newActionState,
        bytes32 indexed newDepositState,
        bytes32 newWithdrawState,
        uint64 newDepositNonce
    );

    event BridgeWithdrawClaimed(
        bytes32 indexed nullifier,
        bytes32 indexed withdrawLeaf,
        bytes32 indexed withdrawState,
        address token,
        address recipient,
        bytes32 zekoAmount,
        uint256 ethereumAmount
    );

    // -------------------------------------------------------------------------
    // Initialization
    // -------------------------------------------------------------------------

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    function initialize(
        address initialAdmin,
        address settlementVerifier_,
        address bridgeVerifier_,
        bytes32 bridgeProgramVKey_,
        address withdrawVerifier_,
        bytes32 withdrawProgramVKey_
    ) external initializer {
        if (initialAdmin == address(0)) revert ZeroAddress();
        if (settlementVerifier_ == address(0)) revert ZeroAddress();
        if (bridgeVerifier_ == address(0)) revert ZeroAddress();
        if (withdrawVerifier_ == address(0)) revert ZeroAddress();

        settlementVerifier = IZekoSettlementVerifier(settlementVerifier_);
        bridgeVerifier = ISP1Verifier(bridgeVerifier_);
        bridgeProgramVKey = bridgeProgramVKey_;
        withdrawVerifier = ISP1Verifier(withdrawVerifier_);
        withdrawProgramVKey = withdrawProgramVKey_;
        currentDepositState = INITIAL_DEPOSIT_STATE;
        currentWithdrawState = bytes32(0);
        depositStateByNonce[0] = INITIAL_DEPOSIT_STATE;

        _grantRole(DEFAULT_ADMIN_ROLE, initialAdmin);
        _grantRole(ADMIN_ROLE, initialAdmin);
        _grantRole(PROVER_ROLE, initialAdmin);
        _grantRole(UPGRADER_ROLE, initialAdmin);

        allowedToken[address(0)] = TokenConfig({
            zekoDecimals: MAX_ZEKO_DECIMALS,
            ethereumDecimals: NATIVE_ETHEREUM_DECIMALS,
            allowed: true
        });

        emit TokenAllowed(
            address(0),
            true,
            MAX_ZEKO_DECIMALS,
            NATIVE_ETHEREUM_DECIMALS
        );
    }

    // -------------------------------------------------------------------------
    // Admin
    // -------------------------------------------------------------------------

    function addToken(
        address token,
        bool allowed,
        uint8 zekoDecimals,
        uint8 ethereumDecimals
    ) external onlyRole(ADMIN_ROLE) {
        TokenConfig memory existingConfig = allowedToken[token];
        if (existingConfig.ethereumDecimals != 0) {
            revert TokenAlreadyAdded(token);
        }

        if (zekoDecimals > MAX_ZEKO_DECIMALS) {
            revert InvalidZekoDecimals(zekoDecimals);
        }

        if (token == address(0)) {
            if (ethereumDecimals != NATIVE_ETHEREUM_DECIMALS) {
                revert InvalidNativeEthereumDecimals(ethereumDecimals);
            }
        } else {
            uint8 actualEthereumDecimals = IERC20Metadata(token).decimals();
            if (actualEthereumDecimals != ethereumDecimals) {
                revert InvalidEthereumDecimals(
                    token,
                    ethereumDecimals,
                    actualEthereumDecimals
                );
            }
        }

        allowedToken[token] = TokenConfig({
            zekoDecimals: zekoDecimals,
            ethereumDecimals: ethereumDecimals,
            allowed: allowed
        });

        emit TokenAllowed(token, allowed, zekoDecimals, ethereumDecimals);
    }

    function setTokenAllowed(
        address token,
        bool allowed
    ) external onlyRole(ADMIN_ROLE) {
        TokenConfig memory existingConfig = allowedToken[token];
        if (existingConfig.ethereumDecimals == 0) revert TokenNotAdded(token);

        allowedToken[token].allowed = allowed;
        emit TokenAllowed(
            token,
            allowed,
            existingConfig.zekoDecimals,
            existingConfig.ethereumDecimals
        );
    }

    function pause() external onlyRole(ADMIN_ROLE) {
        _pause();
    }

    function unpause() external onlyRole(ADMIN_ROLE) {
        _unpause();
    }

    /// @notice Emergency withdrawal for stuck funds.
    /// @dev Use carefully. For a production bridge, prefer a timelock or governance flow.
    function emergencyWithdrawToken(
        address token,
        address to,
        uint256 amount
    ) external onlyRole(ADMIN_ROLE) nonReentrant {
        if (to == address(0)) revert ZeroAddress();
        if (amount == 0) revert ZeroAmount();

        if (token == address(0)) {
            (bool success, ) = payable(to).call{value: amount}("");
            if (!success) revert NativeTransferFailed();
        } else {
            IERC20(token).safeTransfer(to, amount);
        }

        emit EmergencyTokenWithdraw(token, to, amount);
    }

    // -------------------------------------------------------------------------
    // Deposit
    // -------------------------------------------------------------------------

    /// @notice Deposits ERC20 tokens and appends a deposit leaf to the bridge accumulator.
    /// @param token ERC20 token address.
    /// @param amount Token amount to lock on Ethereum.
    /// @param zekoRecipient Packed Zeko recipient address.
    function deposit(
        address token,
        uint256 amount,
        ZekoAddress zekoRecipient,
        uint64 timeout
    )
        external
        nonReentrant
        whenNotPaused
        returns (uint64 nonce, bytes32 depositLeaf, bytes32 newDepositState)
    {
        if (token == address(0)) revert ZeroAddress();

        TokenConfig memory config = allowedToken[token];
        if (!config.allowed) revert TokenNotAllowed(token);
        if (amount == 0) revert ZeroAmount();

        // Transfer first so fee-on-transfer tokens can be rejected by balance delta.
        // For a strict bridge, the received amount must equal the requested amount.
        uint256 balanceBefore = IERC20(token).balanceOf(address(this));
        IERC20(token).safeTransferFrom(msg.sender, address(this), amount);
        uint256 balanceAfter = IERC20(token).balanceOf(address(this));

        uint256 receivedAmount = balanceAfter - balanceBefore;
        if (receivedAmount != amount) revert FeeOnTransferTokenNotSupported();

        return _recordDeposit(token, amount, zekoRecipient, timeout, config);
    }

    /// @notice Deposits native ETH and appends a deposit leaf to the bridge accumulator.
    /// @param zekoRecipient Packed Zeko recipient address.
    /// @param timeout Deadline for the sequencer to relay the deposit to the other side.
    function depositETH(
        ZekoAddress zekoRecipient,
        uint64 timeout
    )
        external
        payable
        nonReentrant
        whenNotPaused
        returns (uint64 nonce, bytes32 depositLeaf, bytes32 newDepositState)
    {
        TokenConfig memory config = allowedToken[address(0)];
        if (!config.allowed) revert TokenNotAllowed(address(0));
        if (msg.value == 0) revert ZeroAmount();

        return
            _recordDeposit(
                address(0),
                msg.value,
                zekoRecipient,
                timeout,
                config
            );
    }

    // -------------------------------------------------------------------------
    // View helpers
    // -------------------------------------------------------------------------

    /// @notice Returns whether a checkpoint exists for a nonce.
    /// @dev Nonce 0 always exists because it is the initial state.
    function hasDepositState(uint64 nonce) external view returns (bool) {
        if (nonce == 0) return depositStateByNonce[0] == INITIAL_DEPOSIT_STATE;
        return
            nonce <= depositNonce && depositStateByNonce[nonce] != bytes32(0);
    }

    /// @notice Returns a historical deposit state, reverting if the nonce does not exist yet.
    function getDepositStateAt(uint64 nonce) external view returns (bytes32) {
        if (nonce > depositNonce) revert InvalidCheckpointNonce(nonce);

        return depositStateByNonce[nonce];
    }

    /// @notice Computes the canonical deposit leaf used by the accumulator.
    function computeDepositLeaf(
        address token,
        ZekoAddress zekoRecipient,
        uint256 zekoAmount,
        uint64 timeout,
        uint64 nonce
    ) public view returns (bytes32) {
        zekoRecipient.unpack();

        return
            keccak256(
                abi.encode(
                    DEPOSIT_LEAF_DOMAIN,
                    block.chainid,
                    address(this),
                    token,
                    zekoRecipient,
                    zekoAmount,
                    timeout,
                    nonce
                )
            );
    }

    /// @notice Computes the next accumulator state from an old state and a deposit leaf.
    function computeNextDepositState(
        bytes32 oldDepositState,
        bytes32 depositLeaf
    ) public pure returns (bytes32) {
        return
            keccak256(
                abi.encode(DEPOSIT_STATE_DOMAIN, oldDepositState, depositLeaf)
            );
    }

    /// @notice Computes the canonical withdraw leaf used by the withdraw accumulator.
    function computeWithdrawLeaf(
        bytes32 token,
        bytes32 recipient,
        bytes32 amount
    ) public view returns (bytes32) {
        return
            keccak256(
                abi.encode(
                    WITHDRAW_LEAF_DOMAIN,
                    block.chainid,
                    address(this),
                    token,
                    recipient,
                    amount
                )
            );
    }

    /// @notice Computes the next withdraw accumulator state from an old state and a withdraw leaf.
    function computeNextWithdrawState(
        bytes32 oldWithdrawState,
        bytes32 withdrawLeaf
    ) public pure returns (bytes32) {
        return
            keccak256(
                abi.encode(
                    WITHDRAW_STATE_DOMAIN,
                    oldWithdrawState,
                    withdrawLeaf
                )
            );
    }

    /// @notice Computes the nullifier consumed when a withdraw is claimed.
    function computeWithdrawNullifier(
        uint64 oldActionStateIndex,
        uint256 withdrawIndex,
        bytes32 withdrawLeaf
    ) public view returns (bytes32) {
        return
            keccak256(
                abi.encode(
                    WITHDRAW_NULLIFIER_DOMAIN,
                    block.chainid,
                    address(this),
                    oldActionStateIndex,
                    withdrawIndex,
                    withdrawLeaf
                )
            );
    }

    function submitBridgeTransition(
        bytes calldata publicValues,
        bytes calldata proofBytes
    ) external onlyRole(PROVER_ROLE) whenNotPaused {
        bridgeVerifier.verifyProof(bridgeProgramVKey, publicValues, proofBytes);

        DecodedBridgePublicValues memory decoded = decodeBridgePublicValues(
            publicValues
        );

        if (
            depositStateByNonce[decoded.ethereumNonceBefore] !=
            decoded.ethereumStateBefore
        ) {
            revert InvalidDepositState(
                depositStateByNonce[decoded.ethereumNonceBefore],
                decoded.ethereumStateBefore
            );
        }
        if (decoded.ethereumNonceAfter != depositNonce) {
            revert InvalidDepositNonce(
                depositNonce,
                decoded.ethereumNonceAfter
            );
        }
        if (decoded.ethereumStateAfter != currentDepositState) {
            revert InvalidDepositState(
                currentDepositState,
                decoded.ethereumStateAfter
            );
        }
        if (
            decoded.ethereumNonceAfter !=
            decoded.ethereumNonceBefore + uint64(decoded.depositCount)
        ) {
            revert InvalidDepositNonce(
                decoded.ethereumNonceBefore + uint64(decoded.depositCount),
                decoded.ethereumNonceAfter
            );
        }
        if (processedActionState[decoded.zekoActionStateAfter]) {
            revert ActionStateAlreadyProcessed(decoded.zekoActionStateAfter);
        }

        processedActionState[decoded.zekoActionStateAfter] = true;

        emit BridgeTransitionAccepted(
            decoded.zekoActionStateBefore,
            decoded.zekoActionStateAfter,
            decoded.ethereumStateAfter,
            currentWithdrawState,
            decoded.ethereumNonceAfter
        );
    }

    function submitWithdrawTransition(
        bytes calldata publicValues,
        bytes calldata proofBytes
    ) external onlyRole(PROVER_ROLE) whenNotPaused {
        withdrawVerifier.verifyProof(
            withdrawProgramVKey,
            publicValues,
            proofBytes
        );

        DecodedWithdrawPublicValues memory decoded = decodeWithdrawPublicValues(
            publicValues
        );

        if (decoded.ethereumWithdrawStateBefore != currentWithdrawState) {
            revert InvalidWithdrawState(decoded.ethereumWithdrawStateBefore);
        }
        if (processedActionState[decoded.zekoActionStateAfter]) {
            revert ActionStateAlreadyProcessed(decoded.zekoActionStateAfter);
        }

        (
            uint64 oldL2ActionStateIndex,
            bool oldL2ActionStateValid
        ) = settlementVerifier.l2ActionStateInfo(decoded.zekoActionStateBefore);
        (
            uint64 newL2ActionStateIndex,
            bool newL2ActionStateValid
        ) = settlementVerifier.l2ActionStateInfo(decoded.zekoActionStateAfter);
        if (!oldL2ActionStateValid) {
            revert InvalidSettlementActionState(decoded.zekoActionStateBefore);
        }
        if (!newL2ActionStateValid) {
            revert InvalidSettlementActionState(decoded.zekoActionStateAfter);
        }
        if (
            oldL2ActionStateIndex != currentWithdrawActionStateIndex ||
            newL2ActionStateIndex != oldL2ActionStateIndex + 1
        ) {
            revert InvalidL2ActionStateTransition(
                decoded.zekoActionStateBefore,
                decoded.zekoActionStateAfter
            );
        }
        if (decoded.withdrawCount > MAX_WITHDRAW_COUNT) {
            revert InvalidWithdrawProof();
        }

        processedActionState[decoded.zekoActionStateAfter] = true;

        if (decoded.withdrawCount > 0) {
            if (
                decoded.withdrawalRoot == bytes32(0) ||
                withdrawalRootInfo[decoded.zekoActionStateBefore].valid
            ) {
                revert InvalidWithdrawProof();
            }

            withdrawalRootInfo[
                decoded.zekoActionStateBefore
            ] = WithdrawalRootInfo({
                withdrawalRoot: decoded.withdrawalRoot,
                withdrawStateBefore: decoded.ethereumWithdrawStateBefore,
                withdrawStateAfter: decoded.ethereumWithdrawStateAfter,
                oldActionStateIndex: oldL2ActionStateIndex,
                withdrawCount: decoded.withdrawCount,
                valid: true
            });

            emit WithdrawalRootAccepted(
                decoded.zekoActionStateBefore,
                decoded.zekoActionStateAfter,
                decoded.withdrawalRoot,
                decoded.ethereumWithdrawStateBefore,
                decoded.ethereumWithdrawStateAfter,
                decoded.withdrawCount
            );
        }

        currentWithdrawState = decoded.ethereumWithdrawStateAfter;
        currentWithdrawActionStateIndex = newL2ActionStateIndex;
    }

    function decodeBridgePublicValues(
        bytes calldata publicValues
    ) public pure returns (DecodedBridgePublicValues memory decoded) {
        if (publicValues.length != BRIDGE_PUBLIC_VALUES_LENGTH) {
            revert InvalidBridgePublicValuesLength(
                BRIDGE_PUBLIC_VALUES_LENGTH,
                publicValues.length
            );
        }

        uint256 cursor = 0;

        decoded.ethereumStateBefore = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.ethereumStateAfter = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.ethereumNonceBefore = _readUint64LE(publicValues, cursor);
        cursor += 8;
        decoded.ethereumNonceAfter = _readUint64LE(publicValues, cursor);
        cursor += 8;
        decoded.zekoActionStateBefore = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.zekoActionStateAfter = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.depositCount = _readUint32LE(publicValues, cursor);
        cursor += 4;

        assert(cursor == BRIDGE_PUBLIC_VALUES_LENGTH);
    }

    function decodeWithdrawPublicValues(
        bytes calldata publicValues
    ) public pure returns (DecodedWithdrawPublicValues memory decoded) {
        if (publicValues.length != WITHDRAW_PUBLIC_VALUES_LENGTH) {
            revert InvalidBridgePublicValuesLength(
                WITHDRAW_PUBLIC_VALUES_LENGTH,
                publicValues.length
            );
        }

        uint256 cursor = 0;

        decoded.zekoActionStateBefore = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.zekoActionStateAfter = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.ethereumWithdrawStateBefore = _readBytes32(
            publicValues,
            cursor
        );
        cursor += 32;
        decoded.ethereumWithdrawStateAfter = _readBytes32(
            publicValues,
            cursor
        );
        cursor += 32;
        decoded.withdrawalRoot = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.withdrawCount = _readUint32LE(publicValues, cursor);
        cursor += 4;

        assert(cursor == WITHDRAW_PUBLIC_VALUES_LENGTH);
    }

    /// @notice Claims a withdraw included in an accepted withdrawal Merkle root.
    /// @param oldActionState Old action state bound to the withdrawal batch.
    /// @param withdraw Clear withdraw being claimed.
    /// @param withdrawIndex Position of `withdraw` inside the withdrawal batch.
    /// @param merkleProof Fixed-depth Merkle proof containing exactly 16 siblings.
    function claimWithdraw(
        bytes32 oldActionState,
        WithdrawClaim calldata withdraw,
        uint256 withdrawIndex,
        bytes32[16] calldata merkleProof
    ) external nonReentrant whenNotPaused {
        WithdrawalRootInfo memory info = withdrawalRootInfo[oldActionState];
        if (!info.valid) revert InvalidWithdrawProof();
        if (withdraw.amount == bytes32(0)) revert ZeroAmount();
        if (withdrawIndex >= info.withdrawCount) revert InvalidWithdrawProof();

        bytes32 withdrawLeaf = computeWithdrawLeaf({
            token: withdraw.token,
            recipient: withdraw.recipient,
            amount: withdraw.amount
        });

        if (
            !_verifyMerkleProof(
                withdrawLeaf,
                withdrawIndex,
                merkleProof,
                info.withdrawalRoot
            )
        ) revert InvalidWithdrawProof();

        bytes32 nullifier = computeWithdrawNullifier(
            info.oldActionStateIndex,
            withdrawIndex,
            withdrawLeaf
        );
        if (spentWithdraw[nullifier]) revert WithdrawAlreadyClaimed(nullifier);
        spentWithdraw[nullifier] = true;

        address token = _fieldAddress(withdraw.token, true);
        TokenConfig memory config = allowedToken[token];
        if (config.ethereumDecimals == 0) revert TokenNotAdded(token);

        address recipient = _recipientAddress(withdraw.recipient);
        uint256 ethereumAmount = _denormalizeAmount(
            uint256(withdraw.amount),
            config,
            token
        );

        if (token == address(0)) {
            (bool success, ) = payable(recipient).call{value: ethereumAmount}(
                ""
            );
            if (!success) revert NativeTransferFailed();
        } else {
            IERC20(token).safeTransfer(recipient, ethereumAmount);
        }

        emit BridgeWithdrawClaimed({
            nullifier: nullifier,
            withdrawLeaf: withdrawLeaf,
            withdrawState: info.withdrawStateAfter,
            token: token,
            recipient: recipient,
            zekoAmount: withdraw.amount,
            ethereumAmount: ethereumAmount
        });
    }

    function _hashMerkleNode(
        bytes32 left,
        bytes32 right
    ) internal pure returns (bytes32) {
        return
            keccak256(abi.encode(WITHDRAW_MERKLE_NODE_DOMAIN, left, right));
    }

    function _verifyMerkleProof(
        bytes32 leaf,
        uint256 index,
        bytes32[16] calldata proof,
        bytes32 root
    ) internal pure returns (bool) {
        bytes32 computed = leaf;

        for (uint256 i = 0; i < WITHDRAW_MERKLE_TREE_DEPTH; i++) {
            bytes32 sibling = proof[i];

            if ((index & 1) == 0) {
                computed = _hashMerkleNode(computed, sibling);
            } else {
                computed = _hashMerkleNode(sibling, computed);
            }

            index >>= 1;
        }

        return computed == root;
    }

    function _recordDeposit(
        address token,
        uint256 amount,
        ZekoAddress zekoRecipient,
        uint64 timeout,
        TokenConfig memory config
    )
        internal
        returns (uint64 nonce, bytes32 depositLeaf, bytes32 newDepositState)
    {
        nonce = depositNonce + 1;

        bytes32 oldDepositState = currentDepositState;
        uint256 zekoAmount = _normalizeAmount(amount, config, token);

        depositLeaf = computeDepositLeaf({
            token: token,
            zekoRecipient: zekoRecipient,
            zekoAmount: zekoAmount,
            timeout: timeout,
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
            amount: amount,
            zekoAmount: zekoAmount,
            timeout: timeout
        });
    }

    function _normalizeAmount(
        uint256 amount,
        TokenConfig memory config,
        address token
    ) internal pure returns (uint256 zekoAmount) {
        if (config.ethereumDecimals == config.zekoDecimals) {
            return amount;
        }

        if (config.ethereumDecimals > config.zekoDecimals) {
            uint8 downscaleDecimals = config.ethereumDecimals -
                config.zekoDecimals;
            uint256 scale = 10 ** downscaleDecimals;
            if (amount % scale != 0) {
                revert InvalidAmountPrecision(
                    token,
                    amount,
                    config.ethereumDecimals,
                    config.zekoDecimals
                );
            }
            return amount / scale;
        }

        uint8 upscaleDecimals = config.zekoDecimals - config.ethereumDecimals;
        return amount * (10 ** upscaleDecimals);
    }

    function _denormalizeAmount(
        uint256 zekoAmount,
        TokenConfig memory config,
        address token
    ) internal pure returns (uint256 ethereumAmount) {
        if (config.ethereumDecimals == config.zekoDecimals) {
            return zekoAmount;
        }

        if (config.ethereumDecimals > config.zekoDecimals) {
            uint8 upscaleDecimals = config.ethereumDecimals -
                config.zekoDecimals;
            return zekoAmount * (10 ** upscaleDecimals);
        }

        uint8 downscaleDecimals = config.zekoDecimals - config.ethereumDecimals;
        uint256 scale = 10 ** downscaleDecimals;
        if (zekoAmount % scale != 0) {
            revert InvalidAmountPrecision(
                token,
                zekoAmount,
                config.ethereumDecimals,
                config.zekoDecimals
            );
        }
        return zekoAmount / scale;
    }

    function _recipientAddress(
        bytes32 recipient
    ) internal pure returns (address) {
        address recipientAddress = _fieldAddress(recipient, false);
        if (recipientAddress == address(0)) revert ZeroAddress();

        return recipientAddress;
    }

    function _fieldAddress(
        bytes32 value,
        bool isToken
    ) internal pure returns (address) {
        if (uint256(value) >> 160 != 0) {
            if (isToken) revert InvalidWithdrawToken(value);
            revert InvalidWithdrawRecipient(value);
        }

        return address(uint160(uint256(value)));
    }

    function _readBytes32(
        bytes calldata data,
        uint256 offset
    ) private pure returns (bytes32 value) {
        assembly {
            value := calldataload(add(data.offset, offset))
        }
    }

    function _readUint64LE(
        bytes calldata data,
        uint256 offset
    ) private pure returns (uint64 value) {
        for (uint256 i = 0; i < 8; i++) {
            value |= uint64(uint8(data[offset + i])) << uint64(8 * i);
        }
    }

    function _readUint32LE(
        bytes calldata data,
        uint256 offset
    ) private pure returns (uint32 value) {
        for (uint256 i = 0; i < 4; i++) {
            value |= uint32(uint8(data[offset + i])) << uint32(8 * i);
        }
    }

    function _authorizeUpgrade(
        address newImplementation
    ) internal view override onlyRole(UPGRADER_ROLE) {
        newImplementation;
    }
}
