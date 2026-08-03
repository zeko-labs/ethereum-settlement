// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {IERC20Metadata} from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Metadata.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {AccessControl} from "@openzeppelin/contracts/access/AccessControl.sol";
import {Initializable} from "@openzeppelin/contracts/proxy/utils/Initializable.sol";
import {UUPSUpgradeable} from "@openzeppelin/contracts/proxy/utils/UUPSUpgradeable.sol";
import {Pausable} from "@openzeppelin/contracts/utils/Pausable.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {ZekoAddress, ZekoAddressLib} from "./ZekoAddress.sol";
import {AssetRecord, ZekoAssetRegistry} from "./ZekoAssetRegistry.sol";
import {ISP1Verifier} from "./ZekoSettlement.sol";

interface IZekoSettlementVerifier {
    function actionState() external view returns (bytes32);

    function outerActionStateLength() external view returns (uint32);

    function appendOuterWitnessBatch(bytes32 stateBefore, bytes32 stateAfter, uint32 count) external;

    function currentVirtualSlot() external view returns (uint64);

    function innerActionBatch(uint64 sequence)
        external
        view
        returns (
            bytes32 minaStateBefore,
            bytes32 minaStateAfter,
            bytes32 root,
            uint32 startIndex,
            uint32 count,
            uint32 commitSlotUpper,
            bool valid
        );

    function isActionStateValid(bytes32 actionState) external view returns (bool);

    function l2ActionStateInfo(bytes32 actionState) external view returns (uint64 index, bool valid);
}

/// @title EthereumZekoBridge
/// @notice Ethereum-side bridge contract for Zeko.
/// @dev Each deposit updates an append-only sequential state:
///      newDepositState = keccak256(DEPOSIT_STATE_DOMAIN, oldDepositState, depositLeaf)
contract EthereumZekoBridge is Initializable, AccessControl, UUPSUpgradeable, Pausable, ReentrancyGuard {
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
    error InvalidAmountPrecision(address token, uint256 amount, uint8 ethereumDecimals, uint8 zekoDecimals);
    error NativeTransferFailed();
    error TokenAlreadyAdded(address token);
    error TokenNotAdded(address token);
    error CanonicalRegistrationRequiresRegistry();
    error CanonicalRecordNotBound(address token);
    error CanonicalTokenStatusRequiresRegistry(address token);
    error InvalidZekoTokenId(bytes32 tokenId);
    error TokenDecimalsMustMatch(uint8 zekoDecimals, uint8 ethereumDecimals);
    error AmountExceedsZekoUInt64(uint256 amount);
    error TokenDepositCapExceeded(address token, uint256 cap, uint256 requestedLiability);
    error CanonicalTokenRequiresSubmitDeposit(address token);
    error InvalidSettlementActionState(bytes32 actionState);
    error InvalidL2ActionStateTransition(bytes32 oldActionState, bytes32 newActionState);
    error ActionStateAlreadyProcessed(bytes32 actionState);
    error InvalidBridgePublicValuesLength(uint256 expected, uint256 actual);
    error InvalidBridgePublicValuesMagic(bytes4 actual);
    error InvalidBridgePublicValuesVersion(uint16 actual);
    error InvalidDepositState(bytes32 expected, bytes32 actual);
    error InvalidDepositNonce(uint64 expected, uint64 actual);
    error InvalidWithdrawState(bytes32 withdrawState);
    error InvalidWithdrawProof();
    error InvalidWithdrawToken(bytes32 token);
    error InvalidWithdrawRecipient(bytes32 recipient);
    error WithdrawAlreadyClaimed(bytes32 nullifier);
    error LegacyDepositPathDisabled();
    error LegacyWithdrawPathDisabled();
    error WithdrawalNotYetClaimable(uint64 currentSlot, uint64 claimableSlot);
    error WithdrawalIndexAlreadyProcessed(address recipient, uint32 currentIndex, uint32 suppliedIndex);
    error InsufficientNativeEscrow(uint256 available, uint256 requested);
    error InsufficientTokenEscrow(address token, uint256 available, uint256 requested);
    error InsufficientExcessTokenBalance(address token, uint256 available, uint256 requested);
    error TokenWithdrawalIndexAlreadyProcessed(
        address token, address recipient, uint32 currentIndex, uint32 suppliedIndex
    );
    error UnauthorizedAssetRegistryModule(address caller);

    // -------------------------------------------------------------------------
    // Constants
    // -------------------------------------------------------------------------

    bytes32 public constant INITIAL_DEPOSIT_STATE = keccak256("ZEKO_BRIDGE_INITIAL_DEPOSIT_STATE_V1");

    bytes32 public constant DEPOSIT_LEAF_DOMAIN = keccak256("ZEKO_BRIDGE_DEPOSIT_LEAF_V1");

    bytes32 public constant DEPOSIT_STATE_DOMAIN = keccak256("ZEKO_BRIDGE_DEPOSIT_STATE_V1");

    bytes32 public constant ERC20_ASSET_V1_DOMAIN = keccak256("ZEKO_ERC20_ASSET_V1");

    bytes32 public constant ERC20_DEPOSIT_LEAF_V2_DOMAIN = keccak256("ZEKO_ERC20_DEPOSIT_LEAF_V2");
    bytes32 public constant ERC20_DEPOSIT_LEAF_V3_DOMAIN = keccak256("ZEKO_ERC20_DEPOSIT_LEAF_V3");

    bytes32 public constant WITHDRAW_LEAF_DOMAIN = keccak256("ZEKO_BRIDGE_WITHDRAW_LEAF_V1");

    bytes32 public constant WITHDRAW_STATE_DOMAIN = keccak256("ZEKO_BRIDGE_WITHDRAW_STATE_V1");

    bytes32 public constant WITHDRAW_NULLIFIER_DOMAIN = keccak256("ZEKO_BRIDGE_WITHDRAW_NULLIFIER_V1");

    bytes32 public constant WITHDRAW_MERKLE_NODE_DOMAIN = keccak256("ZEKO_BRIDGE_WITHDRAW_MERKLE_NODE_V1");

    bytes32 public constant NATIVE_WITHDRAWAL_LEAF_V2_DOMAIN = keccak256("ZEKO_NATIVE_WITHDRAWAL_LEAF_V2");

    bytes32 public constant ERC20_WITHDRAWAL_LEAF_V3_DOMAIN = keccak256("ZEKO_ERC20_WITHDRAWAL_LEAF_V3");
    bytes32 public constant ERC20_WITHDRAWAL_LEAF_V4_DOMAIN = keccak256("ZEKO_ERC20_WITHDRAWAL_LEAF_V4");

    bytes32 public constant INNER_ACTION_NODE_V2_DOMAIN = keccak256("ZEKO_INNER_ACTION_NODE_V2");

    uint256 public constant WITHDRAW_MERKLE_TREE_DEPTH = 16;
    uint256 public constant MAX_WITHDRAW_COUNT = 2 ** WITHDRAW_MERKLE_TREE_DEPTH;

    uint256 private constant BRIDGE_PUBLIC_VALUES_LENGTH = 148;
    bytes4 private constant BRIDGE_PUBLIC_VALUES_V2_MAGIC = 0x5a4b4252; // ZKBR
    uint16 private constant BRIDGE_PUBLIC_VALUES_V2_VERSION = 2;
    uint256 private constant BRIDGE_PUBLIC_VALUES_V2_HEADER_LENGTH = 164;
    uint256 private constant BRIDGE_ACTION_BYTES = 192;
    uint256 private constant WITHDRAW_PUBLIC_VALUES_LENGTH = 164;

    uint8 public constant MAX_ZEKO_DECIMALS = 9;
    uint8 public constant NATIVE_ETHEREUM_DECIMALS = 18;
    uint32 public constant ERC20_ACTION_ENCODING_V2 = 2;

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
        uint16 schemaVersion;
        bytes32 ethereumStateBefore;
        bytes32 ethereumStateAfter;
        uint64 ethereumNonceBefore;
        uint64 ethereumNonceAfter;
        bytes32 zekoActionStateBefore;
        bytes32 zekoActionStateAfter;
        uint32 zekoActionStateLengthBefore;
        uint32 zekoActionStateLengthAfter;
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

    /// @notice Current Ethereum withdrawal state.
    bytes32 public currentWithdrawState;

    /// @notice L2 action-state index matched by the current withdrawal state.
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

    // V2 native bridge storage. Appended for UUPS layout compatibility.
    uint64 public bridgedDepositNonce;
    mapping(address => uint32) public nextWithdrawalIndex;
    uint32 public withdrawalDelaySlots;
    uint256 public nativeEscrowLiability;
    bool public legacyWithdrawEnabled;
    bool public legacyDepositEnabled;

    // Canonical ERC-20 bridge storage. Appended for UUPS layout compatibility.
    mapping(address => bytes32) public zekoTokenIdByToken;
    mapping(address => bytes32) public zekoTokenOwnerByToken;
    mapping(address => bytes32) public assetIdByToken;
    mapping(address => bool) public canonicalTokenRegistered;
    mapping(address => uint256) public escrowLiabilityByToken;
    mapping(address => mapping(address => uint32)) public nextTokenWithdrawalIndex;
    mapping(address => uint64) public depositCapByToken;
    mapping(address => uint32) public registryIndexByToken;
    mapping(address => bytes32) public recordCommitmentByToken;

    /// @notice Registry facet used by this implementation.
    /// @dev Immutable on the implementation, so upgrades select their facet
    /// without consuming or mutating proxy storage.
    ZekoAssetRegistry public immutable assetRegistryModule;

    // -------------------------------------------------------------------------
    // Events
    // -------------------------------------------------------------------------

    event TokenAllowed(address indexed token, bool allowed, uint8 zekoDecimals, uint8 ethereumDecimals);

    event TokenRegistered(
        address indexed token,
        bytes32 indexed assetId,
        bytes32 zekoTokenOwner,
        bytes32 indexed zekoTokenId,
        uint8 decimals,
        uint64 depositCap
    );

    event ERC20DepositSubmitted(
        uint64 indexed nonce,
        bytes32 indexed assetId,
        bytes32 indexed depositLeaf,
        bytes32 newDepositState,
        address token,
        address sender,
        ZekoAddress zekoRecipient,
        uint64 amount,
        uint64 timeout
    );
    event ERC20DepositSubmittedV2(
        uint64 indexed nonce,
        bytes32 indexed assetId,
        bytes32 indexed depositLeaf,
        bytes32 newDepositState,
        address token,
        address sender,
        ZekoAddress zekoRecipient,
        uint64 amount,
        uint64 timeout,
        uint32 encodingVersion,
        uint32 registryIndex,
        bytes32 recordCommitment
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

    event EmergencyTokenWithdraw(address indexed token, address indexed to, uint256 amount);

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
    event NativeWithdrawalClaimed(
        uint64 indexed settlementSequence,
        uint32 indexed globalActionIndex,
        address indexed recipient,
        uint64 zekoAmount,
        uint256 ethereumAmount,
        bytes32 actionFieldsHash
    );
    event ERC20WithdrawalClaimed(
        uint64 indexed settlementSequence,
        uint32 indexed globalActionIndex,
        address indexed token,
        bytes32 assetId,
        address recipient,
        uint64 amount,
        bytes32 actionFieldsHash
    );
    event ERC20WithdrawalClaimedV2(
        uint64 indexed settlementSequence,
        uint32 indexed globalActionIndex,
        address indexed token,
        bytes32 assetId,
        uint32 registryIndex,
        bytes32 recordCommitment,
        address recipient,
        uint64 amount,
        bytes32 actionFieldsHash
    );
    event WithdrawalDelayUpdated(uint32 oldDelay, uint32 newDelay);
    event LegacyDepositPathUpdated(bool enabled);
    event LegacyWithdrawPathUpdated(bool enabled);

    // -------------------------------------------------------------------------
    // Initialization
    // -------------------------------------------------------------------------

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor(ZekoAssetRegistry assetRegistryModule_) {
        if (address(assetRegistryModule_) == address(0)) revert ZeroAddress();
        assetRegistryModule = assetRegistryModule_;
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
        if (initialAdmin == address(0)) {
            revert ZeroAddress();
        }
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
        withdrawalDelaySlots = 20;
        depositStateByNonce[0] = INITIAL_DEPOSIT_STATE;

        _grantRole(DEFAULT_ADMIN_ROLE, initialAdmin);
        _grantRole(ADMIN_ROLE, initialAdmin);
        _grantRole(PROVER_ROLE, initialAdmin);
        _grantRole(UPGRADER_ROLE, initialAdmin);

        allowedToken[address(0)] =
            TokenConfig({zekoDecimals: MAX_ZEKO_DECIMALS, ethereumDecimals: NATIVE_ETHEREUM_DECIMALS, allowed: true});

        emit TokenAllowed(address(0), true, MAX_ZEKO_DECIMALS, NATIVE_ETHEREUM_DECIMALS);
    }

    // -------------------------------------------------------------------------
    // Admin
    // -------------------------------------------------------------------------

    function addToken(address token, bool allowed, uint8 zekoDecimals, uint8 ethereumDecimals)
        external
        onlyRole(ADMIN_ROLE)
    {
        TokenConfig memory existingConfig = allowedToken[token];
        if (existingConfig.allowed) {
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
                revert InvalidEthereumDecimals(token, ethereumDecimals, actualEthereumDecimals);
            }
        }

        allowedToken[token] =
            TokenConfig({zekoDecimals: zekoDecimals, ethereumDecimals: ethereumDecimals, allowed: allowed});

        emit TokenAllowed(token, allowed, zekoDecimals, ethereumDecimals);
    }

    function setTokenAllowed(address token, bool allowed) external onlyRole(ADMIN_ROLE) {
        if (canonicalTokenRegistered[token] && recordCommitmentByToken[token] != bytes32(0)) {
            revert CanonicalTokenStatusRequiresRegistry(token);
        }
        TokenConfig memory existingConfig = allowedToken[token];
        if (existingConfig.ethereumDecimals == 0) revert TokenNotAdded(token);

        allowedToken[token].allowed = allowed;
        emit TokenAllowed(token, allowed, existingConfig.zekoDecimals, existingConfig.ethereumDecimals);
    }

    /// @notice Retained pre-registry one-token registration for fixtures only.
    /// @dev It is available only while legacy deposits are explicitly enabled
    /// and never writes universal registry index/commitment state.
    function registerToken(
        address token,
        bytes32 zekoTokenOwner,
        bytes32 zekoTokenId,
        uint8 zekoDecimals,
        uint8 ethereumDecimals,
        uint64 depositCap
    ) external onlyRole(ADMIN_ROLE) {
        if (!legacyDepositEnabled) {
            revert CanonicalRegistrationRequiresRegistry();
        }
        if (token == address(0)) revert ZeroAddress();
        if (zekoTokenId == bytes32(0)) {
            revert InvalidZekoTokenId(zekoTokenId);
        }
        if (zekoTokenOwner == bytes32(0)) revert ZeroAddress();
        if (depositCap == 0) revert ZeroAmount();
        if (canonicalTokenRegistered[token] || allowedToken[token].allowed) {
            revert TokenAlreadyAdded(token);
        }
        if (zekoDecimals != ethereumDecimals) {
            revert TokenDecimalsMustMatch(zekoDecimals, ethereumDecimals);
        }

        uint8 actualEthereumDecimals = IERC20Metadata(token).decimals();
        if (actualEthereumDecimals != ethereumDecimals) {
            revert InvalidEthereumDecimals(token, ethereumDecimals, actualEthereumDecimals);
        }

        canonicalTokenRegistered[token] = true;
        zekoTokenOwnerByToken[token] = zekoTokenOwner;
        zekoTokenIdByToken[token] = zekoTokenId;
        depositCapByToken[token] = depositCap;
        bytes32 assetId = computeERC20AssetId(token, zekoTokenOwner, zekoTokenId, ethereumDecimals);
        assetIdByToken[token] = assetId;
        allowedToken[token] =
            TokenConfig({zekoDecimals: zekoDecimals, ethereumDecimals: ethereumDecimals, allowed: true});

        emit TokenAllowed(token, true, zekoDecimals, ethereumDecimals);
        emit TokenRegistered(token, assetId, zekoTokenOwner, zekoTokenId, zekoDecimals, depositCap);
    }

    /// @notice Applies a proof-checked registry record to bridge custody state.
    /// @dev Called only by the registry facet through the proxy itself.
    function activateAssetRecordFromRegistry(AssetRecord calldata record, bytes32 recordCommitment) external {
        if (msg.sender != address(this)) {
            revert UnauthorizedAssetRegistryModule(msg.sender);
        }
        address token = record.ethereumToken;
        if (canonicalTokenRegistered[token]) {
            revert TokenAlreadyAdded(token);
        }
        if (recordCommitment == bytes32(0)) {
            revert CanonicalRecordNotBound(token);
        }
        canonicalTokenRegistered[token] = true;
        zekoTokenOwnerByToken[token] = record.tokenOwnerL2;
        zekoTokenIdByToken[token] = record.tokenIdL2;
        assetIdByToken[token] = record.assetId;
        depositCapByToken[token] = record.inventoryCap;
        registryIndexByToken[token] = record.registryIndex;
        recordCommitmentByToken[token] = recordCommitment;
        allowedToken[token] =
            TokenConfig({zekoDecimals: record.decimals, ethereumDecimals: record.decimals, allowed: true});

        emit TokenAllowed(token, true, record.decimals, record.decimals);
        emit TokenRegistered(
            token, record.assetId, record.tokenOwnerL2, record.tokenIdL2, record.decimals, record.inventoryCap
        );
    }

    /// @notice Applies an operational registry status to deposit admission.
    /// @dev Called only by the registry facet through the proxy itself.
    function setAssetAllowedFromRegistry(address token, bool allowed) external {
        if (msg.sender != address(this)) {
            revert UnauthorizedAssetRegistryModule(msg.sender);
        }
        allowedToken[token].allowed = allowed;
        emit TokenAllowed(token, allowed, allowedToken[token].zekoDecimals, allowedToken[token].ethereumDecimals);
    }

    function pause() external onlyRole(ADMIN_ROLE) {
        _pause();
    }

    function unpause() external onlyRole(ADMIN_ROLE) {
        _unpause();
    }

    function setWithdrawalDelaySlots(uint32 newDelay) external onlyRole(ADMIN_ROLE) {
        uint32 oldDelay = withdrawalDelaySlots;
        withdrawalDelaySlots = newDelay;
        emit WithdrawalDelayUpdated(oldDelay, newDelay);
    }

    /// @notice Compatibility switch for pre-V2 fixtures only. New deployments
    /// leave this disabled and use settlement-bound inner-action roots.
    function setLegacyWithdrawEnabled(bool enabled) external onlyRole(ADMIN_ROLE) {
        legacyWithdrawEnabled = enabled;
        emit LegacyWithdrawPathUpdated(enabled);
    }

    /// @notice Compatibility switch for arbitrary-timeout/ERC20 deposit
    /// fixtures. Native PoC deployments leave this disabled so an unsupported
    /// deposit cannot block the canonical nonce stream.
    function setLegacyDepositEnabled(bool enabled) external onlyRole(ADMIN_ROLE) {
        legacyDepositEnabled = enabled;
        emit LegacyDepositPathUpdated(enabled);
    }

    /// @notice Emergency withdrawal for stuck funds.
    /// @dev Use carefully. For a production bridge, prefer a timelock or governance flow.
    function emergencyWithdrawToken(address token, address to, uint256 amount)
        external
        onlyRole(ADMIN_ROLE)
        nonReentrant
    {
        if (to == address(0)) revert ZeroAddress();
        if (amount == 0) revert ZeroAmount();

        if (token == address(0)) {
            (bool success,) = payable(to).call{value: amount}("");
            if (!success) revert NativeTransferFailed();
        } else {
            uint256 tokenBalance = IERC20(token).balanceOf(address(this));
            uint256 liability = escrowLiabilityByToken[token];
            uint256 available = tokenBalance > liability ? tokenBalance - liability : 0;
            if (amount > available) {
                revert InsufficientExcessTokenBalance(token, available, amount);
            }
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
    function deposit(address token, uint256 amount, ZekoAddress zekoRecipient, uint64 timeout)
        external
        nonReentrant
        whenNotPaused
        returns (uint64 nonce, bytes32 depositLeaf, bytes32 newDepositState)
    {
        if (!legacyDepositEnabled) revert LegacyDepositPathDisabled();
        if (token == address(0)) revert ZeroAddress();
        if (canonicalTokenRegistered[token]) {
            revert CanonicalTokenRequiresSubmitDeposit(token);
        }

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

    /// @notice Canonical ERC-20 deposit consumed by the bridge SP1 guest and
    /// converted into a Zeko outer witness action.
    function submitDeposit(address token, uint256 amount, ZekoAddress zekoRecipient)
        external
        nonReentrant
        whenNotPaused
        returns (uint64 nonce, bytes32 depositLeaf, bytes32 newDepositState)
    {
        if (!canonicalTokenRegistered[token]) {
            revert TokenNotAdded(token);
        }
        bytes32 recordCommitment = recordCommitmentByToken[token];
        bool legacyEncoding = recordCommitment == bytes32(0);
        if (legacyEncoding && !legacyDepositEnabled) {
            revert LegacyDepositPathDisabled();
        }
        TokenConfig memory config = allowedToken[token];
        if (!config.allowed) revert TokenNotAllowed(token);
        if (amount == 0) revert ZeroAmount();
        if (amount > type(uint64).max) revert AmountExceedsZekoUInt64(amount);
        // `amount` is bounded to the Zeko UInt64 range above.
        // forge-lint: disable-next-line(unsafe-typecast)
        uint64 zekoAmount = uint64(amount);
        uint256 requestedLiability = escrowLiabilityByToken[token] + amount;
        uint256 depositCap = depositCapByToken[token];
        if (requestedLiability > depositCap) {
            revert TokenDepositCapExceeded(token, depositCap, requestedLiability);
        }

        uint256 balanceBefore = IERC20(token).balanceOf(address(this));
        IERC20(token).safeTransferFrom(msg.sender, address(this), amount);
        uint256 balanceAfter = IERC20(token).balanceOf(address(this));
        if (balanceAfter - balanceBefore != amount) {
            revert FeeOnTransferTokenNotSupported();
        }

        return legacyEncoding
            ? _recordLegacyERC20Deposit(token, zekoAmount, zekoRecipient)
            : _recordERC20Deposit(token, zekoAmount, zekoRecipient);
    }

    /// @notice Deposits native ETH and appends a deposit leaf to the bridge accumulator.
    /// @param zekoRecipient Packed Zeko recipient address.
    /// @param timeout Deadline for the sequencer to relay the deposit to the other side.
    function depositETH(ZekoAddress zekoRecipient, uint64 timeout)
        external
        payable
        nonReentrant
        whenNotPaused
        returns (uint64 nonce, bytes32 depositLeaf, bytes32 newDepositState)
    {
        if (!legacyDepositEnabled) revert LegacyDepositPathDisabled();
        TokenConfig memory config = allowedToken[address(0)];
        if (!config.allowed) revert TokenNotAllowed(address(0));
        if (msg.value == 0) revert ZeroAmount();

        return _recordDeposit(address(0), msg.value, zekoRecipient, timeout, config);
    }

    /// @notice Canonical native bridge deposit. The PoC deliberately has no
    /// cancellation path, so timeout is fixed to Mina's maximum slot.
    function depositETH(ZekoAddress zekoRecipient)
        external
        payable
        nonReentrant
        whenNotPaused
        returns (uint64 nonce, bytes32 depositLeaf, bytes32 newDepositState)
    {
        TokenConfig memory config = allowedToken[address(0)];
        if (!config.allowed) revert TokenNotAllowed(address(0));
        if (msg.value == 0) revert ZeroAmount();
        return _recordDeposit(address(0), msg.value, zekoRecipient, type(uint32).max, config);
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
        ZekoAddress zekoRecipient,
        uint256 zekoAmount,
        uint64 timeout,
        uint64 nonce
    ) public view returns (bytes32) {
        zekoRecipient.unpack();

        return keccak256(
            abi.encode(
                DEPOSIT_LEAF_DOMAIN, block.chainid, address(this), token, zekoRecipient, zekoAmount, timeout, nonce
            )
        );
    }

    function computeERC20AssetId(address token, bytes32 zekoTokenOwner, bytes32 zekoTokenId, uint8 decimals)
        public
        view
        returns (bytes32)
    {
        return keccak256(
            abi.encode(
                ERC20_ASSET_V1_DOMAIN, block.chainid, address(this), token, zekoTokenOwner, zekoTokenId, decimals
            )
        );
    }

    /// @notice Retained V1 one-token leaf for historical fixtures only.
    function computeLegacyERC20DepositLeaf(
        address token,
        bytes32 assetId,
        ZekoAddress zekoRecipient,
        uint64 amount,
        uint64 timeout,
        uint64 nonce
    ) public view returns (bytes32) {
        zekoRecipient.unpack();
        return keccak256(
            abi.encode(
                ERC20_DEPOSIT_LEAF_V2_DOMAIN,
                block.chainid,
                address(this),
                token,
                assetId,
                zekoRecipient,
                amount,
                timeout,
                nonce
            )
        );
    }

    function computeERC20DepositLeaf(
        address token,
        uint32 registryIndex,
        bytes32 recordCommitment,
        bytes32 assetId,
        ZekoAddress zekoRecipient,
        uint64 amount,
        uint64 timeout,
        uint64 nonce
    ) public view returns (bytes32) {
        zekoRecipient.unpack();
        return keccak256(
            abi.encode(
                ERC20_DEPOSIT_LEAF_V3_DOMAIN,
                block.chainid,
                address(this),
                token,
                ERC20_ACTION_ENCODING_V2,
                registryIndex,
                recordCommitment,
                assetId,
                zekoRecipient,
                amount,
                timeout,
                nonce
            )
        );
    }

    /// @notice Computes the next accumulator state from an old state and a deposit leaf.
    function computeNextDepositState(bytes32 oldDepositState, bytes32 depositLeaf) public pure returns (bytes32) {
        return keccak256(abi.encode(DEPOSIT_STATE_DOMAIN, oldDepositState, depositLeaf));
    }

    /// @notice Computes the canonical withdraw leaf used by the withdrawal tree.
    function computeWithdrawLeaf(bytes32 token, bytes32 recipient, bytes32 amount) public view returns (bytes32) {
        return keccak256(abi.encode(WITHDRAW_LEAF_DOMAIN, block.chainid, address(this), token, recipient, amount));
    }

    /// @notice Computes the next withdraw state from an old state and a withdrawal batch commitment.
    function computeNextWithdrawState(bytes32 oldWithdrawState, bytes32 withdrawalRoot, uint32 withdrawCount)
        public
        pure
        returns (bytes32)
    {
        return keccak256(abi.encode(WITHDRAW_STATE_DOMAIN, oldWithdrawState, withdrawalRoot, withdrawCount));
    }

    /// @notice Computes the nullifier consumed when a withdraw is claimed.
    function computeWithdrawNullifier(uint64 oldActionStateIndex, uint256 withdrawIndex, bytes32 withdrawLeaf)
        public
        view
        returns (bytes32)
    {
        return keccak256(
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

    function submitBridgeTransition(bytes calldata publicValues, bytes calldata proofBytes)
        external
        onlyRole(PROVER_ROLE)
        whenNotPaused
    {
        bridgeVerifier.verifyProof(bridgeProgramVKey, publicValues, proofBytes);

        DecodedBridgePublicValues memory decoded = decodeBridgePublicValues(publicValues);

        if (decoded.depositCount == 0) revert InvalidWithdrawProof();
        if (decoded.ethereumNonceBefore != bridgedDepositNonce) {
            revert InvalidDepositNonce(bridgedDepositNonce, decoded.ethereumNonceBefore);
        }
        bytes32 settlementActionState = settlementVerifier.actionState();
        if (decoded.zekoActionStateBefore != settlementActionState) {
            revert InvalidSettlementActionState(decoded.zekoActionStateBefore);
        }
        if (decoded.schemaVersion == BRIDGE_PUBLIC_VALUES_V2_VERSION) {
            uint32 settlementActionStateLength = settlementVerifier.outerActionStateLength();
            if (
                decoded.zekoActionStateLengthBefore != settlementActionStateLength
                    || decoded.zekoActionStateLengthAfter != decoded.zekoActionStateLengthBefore + decoded.depositCount
            ) {
                revert InvalidBridgePublicValuesLength(
                    settlementActionStateLength + decoded.depositCount, decoded.zekoActionStateLengthAfter
                );
            }
        }

        if (depositStateByNonce[decoded.ethereumNonceBefore] != decoded.ethereumStateBefore) {
            revert InvalidDepositState(depositStateByNonce[decoded.ethereumNonceBefore], decoded.ethereumStateBefore);
        }
        if (decoded.ethereumNonceAfter != depositNonce) {
            revert InvalidDepositNonce(depositNonce, decoded.ethereumNonceAfter);
        }
        if (decoded.ethereumStateAfter != currentDepositState) {
            revert InvalidDepositState(currentDepositState, decoded.ethereumStateAfter);
        }
        if (decoded.ethereumNonceAfter != decoded.ethereumNonceBefore + uint64(decoded.depositCount)) {
            revert InvalidDepositNonce(
                decoded.ethereumNonceBefore + uint64(decoded.depositCount), decoded.ethereumNonceAfter
            );
        }
        if (processedActionState[decoded.zekoActionStateAfter]) {
            revert ActionStateAlreadyProcessed(decoded.zekoActionStateAfter);
        }

        processedActionState[decoded.zekoActionStateAfter] = true;
        bridgedDepositNonce = decoded.ethereumNonceAfter;
        if (decoded.schemaVersion == BRIDGE_PUBLIC_VALUES_V2_VERSION) {
            bytes32 stateBefore = decoded.zekoActionStateBefore;
            uint256 actionCursor = BRIDGE_PUBLIC_VALUES_V2_HEADER_LENGTH;
            for (uint32 i = 0; i < decoded.depositCount; i++) {
                bytes32 stateAfter = _readBytes32(publicValues, actionCursor + 160);
                settlementVerifier.appendOuterWitnessBatch(stateBefore, stateAfter, 1);
                stateBefore = stateAfter;
                actionCursor += BRIDGE_ACTION_BYTES;
            }
            if (stateBefore != decoded.zekoActionStateAfter) {
                revert InvalidSettlementActionState(stateBefore);
            }
        } else {
            settlementVerifier.appendOuterWitnessBatch(
                decoded.zekoActionStateBefore, decoded.zekoActionStateAfter, decoded.depositCount
            );
        }

        emit BridgeTransitionAccepted(
            decoded.zekoActionStateBefore,
            decoded.zekoActionStateAfter,
            decoded.ethereumStateAfter,
            currentWithdrawState,
            decoded.ethereumNonceAfter
        );
    }

    function submitWithdrawTransition(bytes calldata publicValues, bytes calldata proofBytes)
        external
        onlyRole(PROVER_ROLE)
        whenNotPaused
    {
        if (!legacyWithdrawEnabled) revert LegacyWithdrawPathDisabled();
        withdrawVerifier.verifyProof(withdrawProgramVKey, publicValues, proofBytes);

        DecodedWithdrawPublicValues memory decoded = decodeWithdrawPublicValues(publicValues);

        if (decoded.ethereumWithdrawStateBefore != currentWithdrawState) {
            revert InvalidWithdrawState(decoded.ethereumWithdrawStateBefore);
        }
        if (processedActionState[decoded.zekoActionStateAfter]) {
            revert ActionStateAlreadyProcessed(decoded.zekoActionStateAfter);
        }

        (uint64 oldL2ActionStateIndex, bool oldL2ActionStateValid) =
            settlementVerifier.l2ActionStateInfo(decoded.zekoActionStateBefore);
        (uint64 newL2ActionStateIndex, bool newL2ActionStateValid) =
            settlementVerifier.l2ActionStateInfo(decoded.zekoActionStateAfter);
        if (!oldL2ActionStateValid) {
            revert InvalidSettlementActionState(decoded.zekoActionStateBefore);
        }
        if (!newL2ActionStateValid) {
            revert InvalidSettlementActionState(decoded.zekoActionStateAfter);
        }
        if (
            oldL2ActionStateIndex != currentWithdrawActionStateIndex
                || newL2ActionStateIndex != oldL2ActionStateIndex + 1
        ) {
            revert InvalidL2ActionStateTransition(decoded.zekoActionStateBefore, decoded.zekoActionStateAfter);
        }
        if (decoded.withdrawCount > MAX_WITHDRAW_COUNT) {
            revert InvalidWithdrawProof();
        }
        if (
            decoded.ethereumWithdrawStateAfter
                != computeNextWithdrawState(
                    decoded.ethereumWithdrawStateBefore, decoded.withdrawalRoot, decoded.withdrawCount
                )
        ) {
            revert InvalidWithdrawProof();
        }

        processedActionState[decoded.zekoActionStateAfter] = true;

        if (decoded.withdrawCount > 0) {
            if (decoded.withdrawalRoot == bytes32(0) || withdrawalRootInfo[decoded.zekoActionStateBefore].valid) {
                revert InvalidWithdrawProof();
            }

            withdrawalRootInfo[decoded.zekoActionStateBefore] = WithdrawalRootInfo({
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

    function decodeBridgePublicValues(bytes calldata publicValues)
        public
        pure
        returns (DecodedBridgePublicValues memory decoded)
    {
        if (publicValues.length == BRIDGE_PUBLIC_VALUES_LENGTH) {
            decoded.schemaVersion = 1;
            uint256 legacyCursor = 0;
            decoded.ethereumStateBefore = _readBytes32(publicValues, legacyCursor);
            legacyCursor += 32;
            decoded.ethereumStateAfter = _readBytes32(publicValues, legacyCursor);
            legacyCursor += 32;
            decoded.ethereumNonceBefore = _readUint64LE(publicValues, legacyCursor);
            legacyCursor += 8;
            decoded.ethereumNonceAfter = _readUint64LE(publicValues, legacyCursor);
            legacyCursor += 8;
            decoded.zekoActionStateBefore = _readBytes32(publicValues, legacyCursor);
            legacyCursor += 32;
            decoded.zekoActionStateAfter = _readBytes32(publicValues, legacyCursor);
            legacyCursor += 32;
            decoded.depositCount = _readUint32LE(publicValues, legacyCursor);
            return decoded;
        }
        if (publicValues.length < BRIDGE_PUBLIC_VALUES_V2_HEADER_LENGTH) {
            revert InvalidBridgePublicValuesLength(BRIDGE_PUBLIC_VALUES_V2_HEADER_LENGTH, publicValues.length);
        }
        bytes4 magic = bytes4(publicValues[0:4]);
        if (magic != BRIDGE_PUBLIC_VALUES_V2_MAGIC) {
            revert InvalidBridgePublicValuesMagic(magic);
        }
        uint16 version = uint16(bytes2(publicValues[4:6]));
        if (version != BRIDGE_PUBLIC_VALUES_V2_VERSION) {
            revert InvalidBridgePublicValuesVersion(version);
        }
        if (publicValues[6] != 0 || publicValues[7] != 0) {
            revert InvalidWithdrawProof();
        }
        decoded.schemaVersion = version;
        uint256 cursor = 8;
        decoded.ethereumStateBefore = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.ethereumStateAfter = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.ethereumNonceBefore = _readUint64BE(publicValues, cursor);
        cursor += 8;
        decoded.ethereumNonceAfter = _readUint64BE(publicValues, cursor);
        cursor += 8;
        decoded.zekoActionStateBefore = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.zekoActionStateAfter = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.zekoActionStateLengthBefore = _readUint32BE(publicValues, cursor);
        cursor += 4;
        decoded.zekoActionStateLengthAfter = _readUint32BE(publicValues, cursor);
        cursor += 4;
        decoded.depositCount = _readUint32BE(publicValues, cursor);
        cursor += 4;
        uint256 expectedLength =
            BRIDGE_PUBLIC_VALUES_V2_HEADER_LENGTH + uint256(decoded.depositCount) * BRIDGE_ACTION_BYTES;
        if (publicValues.length != expectedLength) {
            revert InvalidBridgePublicValuesLength(expectedLength, publicValues.length);
        }
    }

    function decodeWithdrawPublicValues(bytes calldata publicValues)
        public
        pure
        returns (DecodedWithdrawPublicValues memory decoded)
    {
        if (publicValues.length != WITHDRAW_PUBLIC_VALUES_LENGTH) {
            revert InvalidBridgePublicValuesLength(WITHDRAW_PUBLIC_VALUES_LENGTH, publicValues.length);
        }

        uint256 cursor = 0;

        decoded.zekoActionStateBefore = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.zekoActionStateAfter = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.ethereumWithdrawStateBefore = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.ethereumWithdrawStateAfter = _readBytes32(publicValues, cursor);
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
        if (!legacyWithdrawEnabled) {
            revert LegacyWithdrawPathDisabled();
        }
        WithdrawalRootInfo memory info = withdrawalRootInfo[oldActionState];
        if (!info.valid) revert InvalidWithdrawProof();
        if (withdraw.amount == bytes32(0)) revert ZeroAmount();
        if (withdrawIndex >= info.withdrawCount) revert InvalidWithdrawProof();

        bytes32 withdrawLeaf =
            computeWithdrawLeaf({token: withdraw.token, recipient: withdraw.recipient, amount: withdraw.amount});

        if (!_verifyMerkleProof(withdrawLeaf, withdrawIndex, merkleProof, info.withdrawalRoot)) {
            revert InvalidWithdrawProof();
        }

        bytes32 nullifier = computeWithdrawNullifier(info.oldActionStateIndex, withdrawIndex, withdrawLeaf);
        if (spentWithdraw[nullifier]) revert WithdrawAlreadyClaimed(nullifier);
        spentWithdraw[nullifier] = true;

        address token = _fieldAddress(withdraw.token, true);
        TokenConfig memory config = allowedToken[token];
        if (config.ethereumDecimals == 0) revert TokenNotAdded(token);

        address recipient = _recipientAddress(withdraw.recipient);
        uint256 ethereumAmount = _denormalizeAmount(uint256(withdraw.amount), config, token);

        if (token == address(0)) {
            (bool success,) = payable(recipient).call{value: ethereumAmount}("");
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

    /// @notice Claims a native withdrawal directly from the Keccak tree bound
    /// to a real Pickles settlement. No user-generated SNARK is required.
    function claimNativeWithdrawal(
        uint64 settlementSequence,
        uint32 offset,
        address recipient,
        uint64 amount,
        bytes32 actionFieldsHash,
        bytes32[16] calldata merkleProof
    ) external nonReentrant whenNotPaused {
        if (recipient == address(0)) {
            revert ZeroAddress();
        }
        if (amount == 0) revert ZeroAmount();

        (,, bytes32 root, uint32 startIndex, uint32 count, uint32 commitSlotUpper, bool valid) =
            settlementVerifier.innerActionBatch(settlementSequence);
        if (!valid || offset >= count) revert InvalidWithdrawProof();

        uint32 globalActionIndex = startIndex + offset;
        uint32 cursor = nextWithdrawalIndex[recipient];
        if (globalActionIndex < cursor) {
            revert WithdrawalIndexAlreadyProcessed(recipient, cursor, globalActionIndex);
        }

        uint64 currentSlot = settlementVerifier.currentVirtualSlot();
        uint64 claimableSlot = uint64(commitSlotUpper) + uint64(withdrawalDelaySlots);
        if (currentSlot < claimableSlot) {
            revert WithdrawalNotYetClaimable(currentSlot, claimableSlot);
        }

        bytes32 leaf = computeNativeWithdrawalLeaf(globalActionIndex, recipient, amount, actionFieldsHash);
        if (!_verifyInnerActionMerkleProof(leaf, offset, merkleProof, root)) {
            revert InvalidWithdrawProof();
        }

        uint256 ethereumAmount = uint256(amount) * 1 gwei;
        if (nativeEscrowLiability < ethereumAmount) {
            revert InsufficientNativeEscrow(nativeEscrowLiability, ethereumAmount);
        }

        nextWithdrawalIndex[recipient] = globalActionIndex + 1;
        nativeEscrowLiability -= ethereumAmount;
        totalDepositedByToken[address(0)] -= ethereumAmount;
        (bool success,) = payable(recipient).call{value: ethereumAmount}("");
        if (!success) revert NativeTransferFailed();

        emit NativeWithdrawalClaimed(
            settlementSequence, globalActionIndex, recipient, amount, ethereumAmount, actionFieldsHash
        );
    }

    /// @notice Claims a registered ERC-20 withdrawal from the exact inner
    /// action tree committed by a Pickles-backed settlement receipt.
    function claimERC20Withdrawal(
        uint64 settlementSequence,
        uint32 offset,
        address token,
        address recipient,
        uint64 amount,
        bytes32 actionFieldsHash,
        bytes32[16] calldata merkleProof
    ) external nonReentrant whenNotPaused {
        if (!canonicalTokenRegistered[token]) {
            revert TokenNotAdded(token);
        }
        bytes32 recordCommitment = recordCommitmentByToken[token];
        bool legacyEncoding = recordCommitment == bytes32(0);
        if (legacyEncoding && !legacyWithdrawEnabled) {
            revert LegacyWithdrawPathDisabled();
        }
        if (recipient == address(0)) revert ZeroAddress();
        if (amount == 0) revert ZeroAmount();

        (,, bytes32 root, uint32 startIndex, uint32 count, uint32 commitSlotUpper, bool valid) =
            settlementVerifier.innerActionBatch(settlementSequence);
        if (!valid || offset >= count) revert InvalidWithdrawProof();

        uint32 globalActionIndex = startIndex + offset;
        uint32 cursor = nextTokenWithdrawalIndex[token][recipient];
        if (globalActionIndex < cursor) {
            revert TokenWithdrawalIndexAlreadyProcessed(token, recipient, cursor, globalActionIndex);
        }

        uint64 currentSlot = settlementVerifier.currentVirtualSlot();
        uint64 claimableSlot = uint64(commitSlotUpper) + uint64(withdrawalDelaySlots);
        if (currentSlot < claimableSlot) {
            revert WithdrawalNotYetClaimable(currentSlot, claimableSlot);
        }

        bytes32 assetId = assetIdByToken[token];
        uint32 registryIndex = registryIndexByToken[token];
        bytes32 leaf = legacyEncoding
            ? computeLegacyERC20WithdrawalLeaf(globalActionIndex, token, assetId, recipient, amount, actionFieldsHash)
            : computeERC20WithdrawalLeaf(
                globalActionIndex, token, registryIndex, recordCommitment, assetId, recipient, amount, actionFieldsHash
            );
        if (!_verifyInnerActionMerkleProof(leaf, offset, merkleProof, root)) {
            revert InvalidWithdrawProof();
        }

        uint256 liability = escrowLiabilityByToken[token];
        if (liability < amount) {
            revert InsufficientTokenEscrow(token, liability, amount);
        }

        nextTokenWithdrawalIndex[token][recipient] = globalActionIndex + 1;
        escrowLiabilityByToken[token] = liability - amount;
        totalDepositedByToken[token] -= amount;

        uint256 bridgeBalanceBefore = IERC20(token).balanceOf(address(this));
        uint256 recipientBalanceBefore = IERC20(token).balanceOf(recipient);
        IERC20(token).safeTransfer(recipient, amount);
        uint256 bridgeBalanceAfter = IERC20(token).balanceOf(address(this));
        uint256 recipientBalanceAfter = IERC20(token).balanceOf(recipient);
        if (
            bridgeBalanceBefore - bridgeBalanceAfter != amount
                || recipientBalanceAfter - recipientBalanceBefore != amount
        ) revert FeeOnTransferTokenNotSupported();

        emit ERC20WithdrawalClaimed(
            settlementSequence, globalActionIndex, token, assetId, recipient, amount, actionFieldsHash
        );
        if (!legacyEncoding) {
            emit ERC20WithdrawalClaimedV2(
                settlementSequence,
                globalActionIndex,
                token,
                assetId,
                registryIndex,
                recordCommitment,
                recipient,
                amount,
                actionFieldsHash
            );
        }
    }

    function computeNativeWithdrawalLeaf(
        uint32 globalActionIndex,
        address recipient,
        uint64 amount,
        bytes32 actionFieldsHash
    ) public view returns (bytes32) {
        return keccak256(
            abi.encode(
                NATIVE_WITHDRAWAL_LEAF_V2_DOMAIN,
                block.chainid,
                address(this),
                globalActionIndex,
                recipient,
                amount,
                actionFieldsHash
            )
        );
    }

    /// @notice Retained V1 one-token leaf for historical fixtures only.
    function computeLegacyERC20WithdrawalLeaf(
        uint32 globalActionIndex,
        address token,
        bytes32 assetId,
        address recipient,
        uint64 amount,
        bytes32 actionFieldsHash
    ) public view returns (bytes32) {
        return keccak256(
            abi.encode(
                ERC20_WITHDRAWAL_LEAF_V3_DOMAIN,
                block.chainid,
                address(this),
                globalActionIndex,
                token,
                assetId,
                recipient,
                amount,
                actionFieldsHash
            )
        );
    }

    function computeERC20WithdrawalLeaf(
        uint32 globalActionIndex,
        address token,
        uint32 registryIndex,
        bytes32 recordCommitment,
        bytes32 assetId,
        address recipient,
        uint64 amount,
        bytes32 actionFieldsHash
    ) public view returns (bytes32) {
        return keccak256(
            abi.encode(
                ERC20_WITHDRAWAL_LEAF_V4_DOMAIN,
                block.chainid,
                address(this),
                globalActionIndex,
                token,
                ERC20_ACTION_ENCODING_V2,
                registryIndex,
                recordCommitment,
                assetId,
                recipient,
                amount,
                actionFieldsHash
            )
        );
    }

    function _verifyInnerActionMerkleProof(bytes32 leaf, uint256 index, bytes32[16] calldata proof, bytes32 root)
        internal
        pure
        returns (bool)
    {
        bytes32 computed = leaf;
        for (uint256 i = 0; i < WITHDRAW_MERKLE_TREE_DEPTH; i++) {
            bytes32 sibling = proof[i];
            computed = (index & 1) == 0
                ? keccak256(abi.encode(INNER_ACTION_NODE_V2_DOMAIN, computed, sibling))
                : keccak256(abi.encode(INNER_ACTION_NODE_V2_DOMAIN, sibling, computed));
            index >>= 1;
        }
        return computed == root;
    }

    function _hashMerkleNode(bytes32 left, bytes32 right) internal pure returns (bytes32) {
        return keccak256(abi.encode(WITHDRAW_MERKLE_NODE_DOMAIN, left, right));
    }

    function _verifyMerkleProof(bytes32 leaf, uint256 index, bytes32[16] calldata proof, bytes32 root)
        internal
        pure
        returns (bool)
    {
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
    ) internal returns (uint64 nonce, bytes32 depositLeaf, bytes32 newDepositState) {
        nonce = depositNonce + 1;

        bytes32 oldDepositState = currentDepositState;
        uint256 zekoAmount = _normalizeAmount(amount, config, token);

        depositLeaf = computeDepositLeaf({
            token: token, zekoRecipient: zekoRecipient, zekoAmount: zekoAmount, timeout: timeout, nonce: nonce
        });

        newDepositState = computeNextDepositState(oldDepositState, depositLeaf);

        depositNonce = nonce;
        currentDepositState = newDepositState;
        depositStateByNonce[nonce] = newDepositState;
        totalDepositedByToken[token] += amount;
        if (token == address(0)) {
            nativeEscrowLiability += amount;
        } else if (canonicalTokenRegistered[token]) {
            escrowLiabilityByToken[token] += amount;
        }

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

    function _recordERC20Deposit(address token, uint64 amount, ZekoAddress zekoRecipient)
        internal
        returns (uint64 nonce, bytes32 depositLeaf, bytes32 newDepositState)
    {
        nonce = depositNonce + 1;
        uint64 timeout = type(uint32).max;
        bytes32 assetId = assetIdByToken[token];
        uint32 registryIndex = registryIndexByToken[token];
        bytes32 recordCommitment = recordCommitmentByToken[token];
        if (recordCommitment == bytes32(0)) {
            revert CanonicalRecordNotBound(token);
        }
        bytes32 oldDepositState = currentDepositState;
        depositLeaf = computeERC20DepositLeaf(
            token, registryIndex, recordCommitment, assetId, zekoRecipient, amount, timeout, nonce
        );
        newDepositState = computeNextDepositState(oldDepositState, depositLeaf);

        depositNonce = nonce;
        currentDepositState = newDepositState;
        depositStateByNonce[nonce] = newDepositState;
        totalDepositedByToken[token] += amount;
        escrowLiabilityByToken[token] += amount;

        emit BridgeDeposit({
            nonce: nonce,
            depositLeaf: depositLeaf,
            newDepositState: newDepositState,
            oldDepositState: oldDepositState,
            token: token,
            sender: msg.sender,
            zekoRecipient: zekoRecipient,
            amount: amount,
            zekoAmount: amount,
            timeout: timeout
        });
        emit ERC20DepositSubmitted({
            nonce: nonce,
            assetId: assetId,
            depositLeaf: depositLeaf,
            newDepositState: newDepositState,
            token: token,
            sender: msg.sender,
            zekoRecipient: zekoRecipient,
            amount: amount,
            timeout: timeout
        });
        emit ERC20DepositSubmittedV2({
            nonce: nonce,
            assetId: assetId,
            depositLeaf: depositLeaf,
            newDepositState: newDepositState,
            token: token,
            sender: msg.sender,
            zekoRecipient: zekoRecipient,
            amount: amount,
            timeout: timeout,
            encodingVersion: ERC20_ACTION_ENCODING_V2,
            registryIndex: registryIndex,
            recordCommitment: recordCommitment
        });
    }

    function _recordLegacyERC20Deposit(address token, uint64 amount, ZekoAddress zekoRecipient)
        internal
        returns (uint64 nonce, bytes32 depositLeaf, bytes32 newDepositState)
    {
        nonce = depositNonce + 1;
        uint64 timeout = type(uint32).max;
        bytes32 assetId = assetIdByToken[token];
        bytes32 oldDepositState = currentDepositState;
        depositLeaf = computeLegacyERC20DepositLeaf(token, assetId, zekoRecipient, amount, timeout, nonce);
        newDepositState = computeNextDepositState(oldDepositState, depositLeaf);

        depositNonce = nonce;
        currentDepositState = newDepositState;
        depositStateByNonce[nonce] = newDepositState;
        totalDepositedByToken[token] += amount;
        escrowLiabilityByToken[token] += amount;

        emit BridgeDeposit({
            nonce: nonce,
            depositLeaf: depositLeaf,
            newDepositState: newDepositState,
            oldDepositState: oldDepositState,
            token: token,
            sender: msg.sender,
            zekoRecipient: zekoRecipient,
            amount: amount,
            zekoAmount: amount,
            timeout: timeout
        });
        emit ERC20DepositSubmitted({
            nonce: nonce,
            assetId: assetId,
            depositLeaf: depositLeaf,
            newDepositState: newDepositState,
            token: token,
            sender: msg.sender,
            zekoRecipient: zekoRecipient,
            amount: amount,
            timeout: timeout
        });
    }

    function _normalizeAmount(uint256 amount, TokenConfig memory config, address token)
        internal
        pure
        returns (uint256 zekoAmount)
    {
        if (config.ethereumDecimals == config.zekoDecimals) {
            return amount;
        }

        if (config.ethereumDecimals > config.zekoDecimals) {
            uint8 downscaleDecimals = config.ethereumDecimals - config.zekoDecimals;
            uint256 scale = 10 ** downscaleDecimals;
            if (amount % scale != 0) {
                revert InvalidAmountPrecision(token, amount, config.ethereumDecimals, config.zekoDecimals);
            }
            return amount / scale;
        }

        uint8 upscaleDecimals = config.zekoDecimals - config.ethereumDecimals;
        return amount * (10 ** upscaleDecimals);
    }

    function _denormalizeAmount(uint256 zekoAmount, TokenConfig memory config, address token)
        internal
        pure
        returns (uint256 ethereumAmount)
    {
        if (config.ethereumDecimals == config.zekoDecimals) {
            return zekoAmount;
        }

        if (config.ethereumDecimals > config.zekoDecimals) {
            uint8 upscaleDecimals = config.ethereumDecimals - config.zekoDecimals;
            return zekoAmount * (10 ** upscaleDecimals);
        }

        uint8 downscaleDecimals = config.zekoDecimals - config.ethereumDecimals;
        uint256 scale = 10 ** downscaleDecimals;
        if (zekoAmount % scale != 0) {
            revert InvalidAmountPrecision(token, zekoAmount, config.ethereumDecimals, config.zekoDecimals);
        }
        return zekoAmount / scale;
    }

    function _recipientAddress(bytes32 recipient) internal pure returns (address) {
        address recipientAddress = _fieldAddress(recipient, false);
        if (recipientAddress == address(0)) revert ZeroAddress();

        return recipientAddress;
    }

    function _fieldAddress(bytes32 value, bool isToken) internal pure returns (address) {
        if (uint256(value) >> 160 != 0) {
            if (isToken) revert InvalidWithdrawToken(value);
            revert InvalidWithdrawRecipient(value);
        }

        return address(uint160(uint256(value)));
    }

    function _readBytes32(bytes calldata data, uint256 offset) private pure returns (bytes32 value) {
        assembly {
            value := calldataload(add(data.offset, offset))
        }
    }

    function _readUint64LE(bytes calldata data, uint256 offset) private pure returns (uint64 value) {
        for (uint8 i = 0; i < 8; i++) {
            value |= uint64(uint8(data[offset + i])) << (8 * i);
        }
    }

    function _readUint32LE(bytes calldata data, uint256 offset) private pure returns (uint32 value) {
        for (uint8 i = 0; i < 4; i++) {
            value |= uint32(uint8(data[offset + i])) << (8 * i);
        }
    }

    function _readUint64BE(bytes calldata data, uint256 offset) private pure returns (uint64) {
        return uint64(bytes8(data[offset:offset + 8]));
    }

    function _readUint32BE(bytes calldata data, uint256 offset) private pure returns (uint32) {
        return uint32(bytes4(data[offset:offset + 4]));
    }

    /// @dev Registry selectors are implemented by the immutable facet while
    /// executing against this proxy's namespaced registry storage.
    fallback() external {
        address module = address(assetRegistryModule);
        assembly ("memory-safe") {
            calldatacopy(0, 0, calldatasize())
            let success := delegatecall(gas(), module, 0, calldatasize(), 0, 0)
            returndatacopy(0, 0, returndatasize())
            switch success
            case 0 {
                revert(0, returndatasize())
            }
            default {
                return(0, returndatasize())
            }
        }
    }

    function _authorizeUpgrade(address newImplementation) internal view override onlyRole(UPGRADER_ROLE) {
        newImplementation;
    }
}
