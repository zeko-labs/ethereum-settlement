// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
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
    error InvalidAmountPrecision(address token, uint256 amount, uint8 ethereumDecimals, uint8 zekoDecimals);
    error NativeTransferFailed();
    error TokenAlreadyAdded(address token);
    error TokenNotAdded(address token);
    error CanonicalRecordNotBound(address token);
    error AmountExceedsZekoUInt64(uint256 amount);
    error TokenDepositCapExceeded(address token, uint256 cap, uint256 requestedLiability);
    error InvalidSettlementActionState(bytes32 actionState);
    error ActionStateAlreadyProcessed(bytes32 actionState);
    error InvalidBridgePublicValuesLength(uint256 expected, uint256 actual);
    error InvalidBridgePublicValuesMagic(bytes4 actual);
    error InvalidBridgePublicValuesVersion(uint16 actual);
    error InvalidDepositState(bytes32 expected, bytes32 actual);
    error InvalidDepositNonce(uint64 expected, uint64 actual);
    error InvalidWithdrawProof();
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

    bytes32 public constant ERC20_DEPOSIT_LEAF_V3_DOMAIN = keccak256("ZEKO_ERC20_DEPOSIT_LEAF_V3");

    bytes32 public constant NATIVE_WITHDRAWAL_LEAF_V2_DOMAIN = keccak256("ZEKO_NATIVE_WITHDRAWAL_LEAF_V2");

    bytes32 public constant ERC20_WITHDRAWAL_LEAF_V4_DOMAIN = keccak256("ZEKO_ERC20_WITHDRAWAL_LEAF_V4");

    bytes32 public constant INNER_ACTION_NODE_V2_DOMAIN = keccak256("ZEKO_INNER_ACTION_NODE_V2");

    uint256 public constant WITHDRAW_MERKLE_TREE_DEPTH = 16;
    bytes4 private constant BRIDGE_PUBLIC_VALUES_V2_MAGIC = 0x5a4b4252; // ZKBR
    uint16 private constant BRIDGE_PUBLIC_VALUES_V2_VERSION = 2;
    uint256 private constant BRIDGE_PUBLIC_VALUES_V2_HEADER_LENGTH = 164;
    uint256 private constant BRIDGE_ACTION_BYTES = 192;

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

    struct DecodedBridgePublicValues {
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

    /// @dev Deprecated separate-withdrawal storage shape retained only so an
    /// upgrade does not reinterpret any existing proxy slots.
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

    /// @dev Deprecated storage retained for UUPS layout compatibility.
    bytes32 private currentWithdrawState;

    /// @dev Deprecated storage retained for UUPS layout compatibility.
    uint64 private currentWithdrawActionStateIndex;

    /// @notice Historical deposit state by nonce.
    /// @dev depositStateByNonce[0] is INITIAL_DEPOSIT_STATE.
    mapping(uint64 => bytes32) public depositStateByNonce;

    /// @notice Settlement action states already consumed by bridge transitions.
    mapping(bytes32 => bool) public processedActionState;

    /// @dev Deprecated storage retained for UUPS layout compatibility.
    mapping(bytes32 => bool) private validWithdrawState;

    /// @dev Deprecated storage retained for UUPS layout compatibility.
    mapping(bytes32 => bytes32) private withdrawStateOldActionState;
    /// @dev Deprecated storage retained for UUPS layout compatibility.
    mapping(bytes32 => uint64) private withdrawStateOldActionStateIndex;

    /// @dev Deprecated storage retained for UUPS layout compatibility.
    mapping(bytes32 => bool) private spentWithdraw;

    /// @notice Token configuration by L1 token address. `address(0)` is native ETH.
    mapping(address => TokenConfig) public allowedToken;

    /// @notice Total deposited amount per token.
    mapping(address => uint256) public totalDepositedByToken;

    IZekoSettlementVerifier public settlementVerifier;
    ISP1Verifier public bridgeVerifier;
    bytes32 public bridgeProgramVKey;
    ISP1Verifier private withdrawVerifier;
    bytes32 private withdrawProgramVKey;

    /// @dev Deprecated storage retained for UUPS layout compatibility.
    mapping(bytes32 => WithdrawalRootInfo) private withdrawalRootInfo;

    // V2 native bridge storage. Appended for UUPS layout compatibility.
    uint64 public bridgedDepositNonce;
    mapping(address => uint32) public nextWithdrawalIndex;
    uint32 public withdrawalDelaySlots;
    uint256 public nativeEscrowLiability;
    bool private legacyWithdrawEnabled;
    bool private legacyDepositEnabled;

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

    event BridgeTransitionAccepted(
        bytes32 indexed oldActionState,
        bytes32 indexed newActionState,
        bytes32 indexed newDepositState,
        uint64 newDepositNonce
    );
    event NativeWithdrawalClaimed(
        uint64 indexed settlementSequence,
        uint32 indexed globalActionIndex,
        address indexed recipient,
        uint64 zekoAmount,
        uint256 ethereumAmount,
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
        bytes32 bridgeProgramVKey_
    ) external initializer {
        if (initialAdmin == address(0)) {
            revert ZeroAddress();
        }
        if (settlementVerifier_ == address(0)) revert ZeroAddress();
        if (bridgeVerifier_ == address(0)) revert ZeroAddress();

        settlementVerifier = IZekoSettlementVerifier(settlementVerifier_);
        bridgeVerifier = ISP1Verifier(bridgeVerifier_);
        bridgeProgramVKey = bridgeProgramVKey_;
        currentDepositState = INITIAL_DEPOSIT_STATE;
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
        if (recordCommitment == bytes32(0)) revert CanonicalRecordNotBound(token);
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

        return _recordERC20Deposit(token, zekoAmount, zekoRecipient);
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
        return _recordNativeDeposit(msg.value, zekoRecipient);
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
        uint32 settlementActionStateLength = settlementVerifier.outerActionStateLength();
        if (
            decoded.zekoActionStateLengthBefore != settlementActionStateLength
                || decoded.zekoActionStateLengthAfter != decoded.zekoActionStateLengthBefore + decoded.depositCount
        ) {
            revert InvalidBridgePublicValuesLength(
                settlementActionStateLength + decoded.depositCount, decoded.zekoActionStateLengthAfter
            );
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

        emit BridgeTransitionAccepted(
            decoded.zekoActionStateBefore,
            decoded.zekoActionStateAfter,
            decoded.ethereumStateAfter,
            decoded.ethereumNonceAfter
        );
    }

    function decodeBridgePublicValues(bytes calldata publicValues)
        public
        pure
        returns (DecodedBridgePublicValues memory decoded)
    {
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
        if (recordCommitment == bytes32(0)) revert CanonicalRecordNotBound(token);
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
        bytes32 leaf = computeERC20WithdrawalLeaf(
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

    function _recordNativeDeposit(uint256 amount, ZekoAddress zekoRecipient)
        internal
        returns (uint64 nonce, bytes32 depositLeaf, bytes32 newDepositState)
    {
        nonce = depositNonce + 1;
        uint64 timeout = type(uint32).max;
        bytes32 oldDepositState = currentDepositState;
        if (amount % 1 gwei != 0) {
            revert InvalidAmountPrecision(address(0), amount, NATIVE_ETHEREUM_DECIMALS, MAX_ZEKO_DECIMALS);
        }
        uint256 zekoAmount = amount / 1 gwei;

        depositLeaf = computeDepositLeaf({
            token: address(0), zekoRecipient: zekoRecipient, zekoAmount: zekoAmount, timeout: timeout, nonce: nonce
        });

        newDepositState = computeNextDepositState(oldDepositState, depositLeaf);

        depositNonce = nonce;
        currentDepositState = newDepositState;
        depositStateByNonce[nonce] = newDepositState;
        totalDepositedByToken[address(0)] += amount;
        nativeEscrowLiability += amount;

        emit BridgeDeposit({
            nonce: nonce,
            depositLeaf: depositLeaf,
            newDepositState: newDepositState,
            oldDepositState: oldDepositState,
            token: address(0),
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

    function _readBytes32(bytes calldata data, uint256 offset) private pure returns (bytes32 value) {
        assembly {
            value := calldataload(add(data.offset, offset))
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
