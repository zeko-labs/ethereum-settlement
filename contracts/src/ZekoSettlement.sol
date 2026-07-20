// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {AccessControl} from "@openzeppelin/contracts/access/AccessControl.sol";
import {Initializable} from "@openzeppelin/contracts/proxy/utils/Initializable.sol";
import {UUPSUpgradeable} from "@openzeppelin/contracts/proxy/utils/UUPSUpgradeable.sol";

interface ISP1Verifier {
    function verifyProof(
        bytes32 programVKey,
        bytes calldata publicValues,
        bytes calldata proofBytes
    ) external view;
}

/// @title ZekoSettlement
/// @notice Ethereum checkpoint contract for normal multisig-DA Zeko commits.
/// @dev The SP1 guest verifies the complete Pickles proof. This contract only
///      enforces the Ethereum domain, state continuity, and virtual-slot clock.
contract ZekoSettlement is Initializable, AccessControl, UUPSUpgradeable {
    bytes4 public constant PUBLIC_VALUES_MAGIC = 0x5a4b5354; // "ZKST"
    uint16 public constant PUBLIC_VALUES_VERSION = 1;
    uint16 public constant PUBLIC_VALUES_V2_VERSION = 2;
    uint8 public constant DA_MODE_MULTISIG = 1;
    uint256 public constant PUBLIC_VALUES_LENGTH = 768;
    uint256 public constant PUBLIC_VALUES_V2_LENGTH = 828;
    uint256 private constant STATE_ARRAY_LENGTH = 8;

    bytes32 public constant ADMIN_ROLE = keccak256("ADMIN_ROLE");
    bytes32 public constant PROVER_ROLE = keccak256("PROVER_ROLE");
    bytes32 public constant BRIDGE_ROLE = keccak256("BRIDGE_ROLE");
    bytes32 public constant UPGRADER_ROLE = keccak256("UPGRADER_ROLE");

    ISP1Verifier public verifier;
    bytes32 public programVKey;

    // Compatibility getters retained for the bridge and existing indexers.
    bytes32 public vkHash;
    bytes32 public actionState;
    bytes32 public currentRoot;
    mapping(bytes32 => bool) public validActionState;
    uint64 public currentL2ActionStateIndex;

    struct L2ActionStateInfo {
        uint64 index;
        bool valid;
    }

    mapping(bytes32 => L2ActionStateInfo) public l2ActionStateInfo;

    // V1 complete settlement state.
    bytes32[8] private _outerState;
    uint32 public outerActionStateLength;
    uint64 public batchSequence;
    uint64 public genesisTimestamp;
    uint32 public slotDuration;
    uint32 public forkSlot;

    struct AcceptedInnerActionState {
        uint64 settlementIndex;
        uint64 acceptedAt;
        uint32 length;
        bool valid;
    }

    mapping(bytes32 => AcceptedInnerActionState) public acceptedInnerActionState;

    // V2 bridge checkpoints. Appended after V1 storage for UUPS compatibility.
    struct OuterActionStateInfo {
        uint32 length;
        bool valid;
    }

    struct InnerActionBatch {
        bytes32 minaStateBefore;
        bytes32 minaStateAfter;
        bytes32 root;
        uint32 startIndex;
        uint32 count;
        uint32 commitSlotUpper;
        bool valid;
    }

    mapping(bytes32 => OuterActionStateInfo) public outerActionStateInfo;
    mapping(uint64 => InnerActionBatch) public innerActionBatch;
    address public bridgeContract;

    struct DecodedPublicValues {
        uint16 version;
        uint8 daMode;
        uint64 chainId;
        address settlementContract;
        uint64 batchSequence;
        bytes32 vkHash;
        bytes32 appStatement;
        bytes32 minaTransactionHash;
        bytes32[8] stateBefore;
        bytes32[8] stateAfter;
        bytes32 outerActionStateBefore;
        bytes32 outerActionStateAfter;
        uint32 outerActionStateLengthBefore;
        uint32 outerActionStateLengthAfter;
        bytes32 synchronizedOuterActionState;
        uint32 synchronizedOuterActionStateLength;
        uint32 slotLower;
        uint32 slotUpper;
        address bridgeAddress;
        bytes32 innerActionRoot;
        uint32 innerActionStartIndex;
        uint32 innerActionCount;
    }

    event VkHashUpdated(bytes32 indexed oldVkHash, bytes32 indexed newVkHash);
    event SettlementAccepted(
        uint64 indexed batchSequence,
        bytes32 indexed minaTransactionHash,
        bytes32 indexed ledgerHash,
        bytes32 outerActionState,
        uint32 outerActionStateLength,
        bytes32 innerActionState,
        uint32 innerActionStateLength,
        uint32 slotLower,
        uint32 slotUpper
    );
    event BridgeContractUpdated(
        address indexed oldBridge,
        address indexed newBridge
    );
    event OuterWitnessBatchAppended(
        bytes32 indexed stateBefore,
        bytes32 indexed stateAfter,
        uint32 count,
        uint32 lengthAfter
    );
    event InnerActionBatchAccepted(
        uint64 indexed batchSequence,
        bytes32 indexed stateAfter,
        bytes32 indexed root,
        uint32 startIndex,
        uint32 count,
        uint32 claimableSlot
    );

    error ZeroAddress();
    error InvalidPublicValuesLength(uint256 expected, uint256 actual);
    error InvalidPublicValuesMagic(bytes4 actual);
    error InvalidPublicValuesVersion(uint16 actual);
    error InvalidReservedByte(uint8 actual);
    error InvalidDaMode(uint8 actual);
    error InvalidChainId(uint64 expected, uint64 actual);
    error InvalidSettlementContract(address expected, address actual);
    error InvalidBatchSequence(uint64 expected, uint64 actual);
    error InvalidVkHash(bytes32 expected, bytes32 actual);
    error InvalidOuterState(uint256 field, bytes32 expected, bytes32 actual);
    error InvalidActionState(bytes32 expected, bytes32 actual);
    error InvalidActionStateLength(uint32 expected, uint32 actual);
    error InvalidActionStateTransition(uint32 beforeLength, uint32 afterLength);
    error InvalidSynchronizedActionLength(uint32 synchronized, uint32 available);
    error UnknownSynchronizedActionState(bytes32 state, uint32 length);
    error InvalidBridgeContract(address expected, address actual);
    error InvalidInnerActionTransition(uint32 beforeLength, uint32 afterLength);
    error InvalidInnerActionBatch(bytes32 root, uint32 startIndex, uint32 count);
    error InvalidSlotRange(uint32 lower, uint32 upper);
    error OutsideSlotRange(uint64 current, uint32 lower, uint32 upper);
    error InvalidSlotDuration();
    error FieldDoesNotFitUint32(bytes32 value);

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    function initialize(
        address initialAdmin,
        address verifier_,
        bytes32 programVKey_,
        bytes32 initialVkHash,
        bytes32[8] calldata initialOuterState,
        bytes32 initialOuterActionState,
        uint32 initialOuterActionStateLength,
        uint64 genesisTimestamp_,
        uint32 slotDuration_,
        uint32 forkSlot_
    ) external initializer {
        if (initialAdmin == address(0) || verifier_ == address(0)) {
            revert ZeroAddress();
        }
        if (slotDuration_ == 0) revert InvalidSlotDuration();

        verifier = ISP1Verifier(verifier_);
        programVKey = programVKey_;
        vkHash = initialVkHash;
        _outerState = initialOuterState;
        actionState = initialOuterActionState;
        currentRoot = initialOuterState[2];
        outerActionStateLength = initialOuterActionStateLength;
        genesisTimestamp = genesisTimestamp_;
        slotDuration = slotDuration_;
        forkSlot = forkSlot_;

        validActionState[initialOuterActionState] = true;
        outerActionStateInfo[initialOuterActionState] = OuterActionStateInfo({
            length: initialOuterActionStateLength,
            valid: true
        });
        l2ActionStateInfo[initialOuterActionState] = L2ActionStateInfo({
            index: 0,
            valid: true
        });
        _recordAcceptedInnerState(initialOuterState[3], initialOuterState[4], 0);

        _grantRole(DEFAULT_ADMIN_ROLE, initialAdmin);
        _grantRole(ADMIN_ROLE, initialAdmin);
        _grantRole(PROVER_ROLE, initialAdmin);
        _grantRole(UPGRADER_ROLE, initialAdmin);

        emit VkHashUpdated(bytes32(0), initialVkHash);
    }

    function outerState() external view returns (bytes32[8] memory) {
        return _outerState;
    }

    function currentVirtualSlot() public view returns (uint64) {
        if (block.timestamp <= genesisTimestamp) return forkSlot;
        return
            uint64(forkSlot) +
            uint64((block.timestamp - genesisTimestamp) / slotDuration);
    }

    function setVkHash(bytes32 newVkHash) external onlyRole(ADMIN_ROLE) {
        bytes32 oldVkHash = vkHash;
        vkHash = newVkHash;
        emit VkHashUpdated(oldVkHash, newVkHash);
    }

    function setBridgeContract(
        address newBridge
    ) external onlyRole(ADMIN_ROLE) {
        if (newBridge == address(0)) revert ZeroAddress();
        address oldBridge = bridgeContract;
        if (oldBridge != address(0)) _revokeRole(BRIDGE_ROLE, oldBridge);
        bridgeContract = newBridge;
        _grantRole(BRIDGE_ROLE, newBridge);
        emit BridgeContractUpdated(oldBridge, newBridge);
    }

    /// @notice Appends a bridge-proved contiguous range of outer Witness
    /// actions. Poseidon is checked by the bridge SP1 program; this function
    /// owns only checkpoint continuity and exact length accounting.
    function appendOuterWitnessBatch(
        bytes32 stateBefore,
        bytes32 stateAfter,
        uint32 count
    ) external onlyRole(BRIDGE_ROLE) {
        if (stateBefore != actionState) {
            revert InvalidActionState(actionState, stateBefore);
        }
        if (count == 0) {
            revert InvalidActionStateTransition(
                outerActionStateLength,
                outerActionStateLength
            );
        }
        uint32 lengthAfter = outerActionStateLength + count;
        OuterActionStateInfo memory existing = outerActionStateInfo[stateAfter];
        if (existing.valid && existing.length != lengthAfter) {
            revert InvalidActionStateLength(existing.length, lengthAfter);
        }

        actionState = stateAfter;
        outerActionStateLength = lengthAfter;
        validActionState[stateAfter] = true;
        outerActionStateInfo[stateAfter] = OuterActionStateInfo({
            length: lengthAfter,
            valid: true
        });
        _recordL2ActionState(stateAfter);

        emit OuterWitnessBatchAppended(
            stateBefore,
            stateAfter,
            count,
            lengthAfter
        );
    }

    function isActionStateValid(
        bytes32 targetActionState
    ) external view returns (bool) {
        return validActionState[targetActionState];
    }

    function verifyAndUpdateRoot(
        bytes calldata publicValues,
        bytes calldata proofBytes
    ) external onlyRole(PROVER_ROLE) {
        verifier.verifyProof(programVKey, publicValues, proofBytes);
        DecodedPublicValues memory decoded = decodePublicValues(publicValues);

        if (decoded.daMode != DA_MODE_MULTISIG) {
            revert InvalidDaMode(decoded.daMode);
        }
        if (decoded.chainId != block.chainid) {
            revert InvalidChainId(uint64(block.chainid), decoded.chainId);
        }
        if (decoded.settlementContract != address(this)) {
            revert InvalidSettlementContract(
                address(this),
                decoded.settlementContract
            );
        }
        uint64 expectedBatch = batchSequence + 1;
        if (decoded.batchSequence != expectedBatch) {
            revert InvalidBatchSequence(expectedBatch, decoded.batchSequence);
        }
        if (decoded.vkHash != vkHash) {
            revert InvalidVkHash(vkHash, decoded.vkHash);
        }
        for (uint256 i = 0; i < STATE_ARRAY_LENGTH; i++) {
            if (decoded.stateBefore[i] != _outerState[i]) {
                revert InvalidOuterState(
                    i,
                    _outerState[i],
                    decoded.stateBefore[i]
                );
            }
        }
        if (decoded.outerActionStateBefore != actionState) {
            revert InvalidActionState(
                actionState,
                decoded.outerActionStateBefore
            );
        }
        if (
            decoded.outerActionStateLengthBefore != outerActionStateLength
        ) {
            revert InvalidActionStateLength(
                outerActionStateLength,
                decoded.outerActionStateLengthBefore
            );
        }
        if (
            decoded.outerActionStateLengthAfter !=
            decoded.outerActionStateLengthBefore + 1
        ) {
            revert InvalidActionStateTransition(
                decoded.outerActionStateLengthBefore,
                decoded.outerActionStateLengthAfter
            );
        }
        if (
            decoded.synchronizedOuterActionStateLength >
            decoded.outerActionStateLengthBefore
        ) {
            revert InvalidSynchronizedActionLength(
                decoded.synchronizedOuterActionStateLength,
                decoded.outerActionStateLengthBefore
            );
        }
        OuterActionStateInfo memory synchronizedCheckpoint = outerActionStateInfo[
            decoded.synchronizedOuterActionState
        ];
        if (
            !synchronizedCheckpoint.valid ||
            synchronizedCheckpoint.length !=
            decoded.synchronizedOuterActionStateLength
        ) {
            revert UnknownSynchronizedActionState(
                decoded.synchronizedOuterActionState,
                decoded.synchronizedOuterActionStateLength
            );
        }
        if (decoded.slotLower > decoded.slotUpper) {
            revert InvalidSlotRange(decoded.slotLower, decoded.slotUpper);
        }
        uint64 currentSlot = currentVirtualSlot();
        if (
            currentSlot < decoded.slotLower || currentSlot > decoded.slotUpper
        ) {
            revert OutsideSlotRange(
                currentSlot,
                decoded.slotLower,
                decoded.slotUpper
            );
        }

        _outerState = decoded.stateAfter;
        actionState = decoded.outerActionStateAfter;
        outerActionStateLength = decoded.outerActionStateLengthAfter;
        batchSequence = decoded.batchSequence;
        currentRoot = decoded.stateAfter[2];
        validActionState[decoded.outerActionStateAfter] = true;
        outerActionStateInfo[
            decoded.outerActionStateAfter
        ] = OuterActionStateInfo({
            length: decoded.outerActionStateLengthAfter,
            valid: true
        });
        _recordL2ActionState(decoded.outerActionStateAfter);
        _recordAcceptedInnerState(
            decoded.stateAfter[3],
            decoded.stateAfter[4],
            decoded.batchSequence
        );

        if (decoded.version == PUBLIC_VALUES_V2_VERSION) {
            if (decoded.bridgeAddress != bridgeContract) {
                revert InvalidBridgeContract(
                    bridgeContract,
                    decoded.bridgeAddress
                );
            }
            uint32 innerLengthBefore = _fieldToUint32(
                decoded.stateBefore[4]
            );
            uint32 innerLengthAfter = _fieldToUint32(decoded.stateAfter[4]);
            if (
                innerLengthAfter < innerLengthBefore ||
                innerLengthAfter - innerLengthBefore !=
                decoded.innerActionCount
            ) {
                revert InvalidInnerActionTransition(
                    innerLengthBefore,
                    innerLengthAfter
                );
            }
            if (
                decoded.innerActionStartIndex != innerLengthBefore ||
                decoded.innerActionRoot == bytes32(0)
            ) {
                revert InvalidInnerActionBatch(
                    decoded.innerActionRoot,
                    decoded.innerActionStartIndex,
                    decoded.innerActionCount
                );
            }
            innerActionBatch[decoded.batchSequence] = InnerActionBatch({
                minaStateBefore: decoded.stateBefore[3],
                minaStateAfter: decoded.stateAfter[3],
                root: decoded.innerActionRoot,
                startIndex: decoded.innerActionStartIndex,
                count: decoded.innerActionCount,
                commitSlotUpper: decoded.slotUpper,
                valid: true
            });
            emit InnerActionBatchAccepted(
                decoded.batchSequence,
                decoded.stateAfter[3],
                decoded.innerActionRoot,
                decoded.innerActionStartIndex,
                decoded.innerActionCount,
                decoded.slotUpper
            );
        }

        emit SettlementAccepted(
            decoded.batchSequence,
            decoded.minaTransactionHash,
            decoded.stateAfter[2],
            decoded.outerActionStateAfter,
            decoded.outerActionStateLengthAfter,
            decoded.stateAfter[3],
            _fieldToUint32(decoded.stateAfter[4]),
            decoded.slotLower,
            decoded.slotUpper
        );
    }

    function decodePublicValues(
        bytes calldata publicValues
    ) public pure returns (DecodedPublicValues memory decoded) {
        if (
            publicValues.length != PUBLIC_VALUES_LENGTH &&
            publicValues.length != PUBLIC_VALUES_V2_LENGTH
        ) {
            revert InvalidPublicValuesLength(
                PUBLIC_VALUES_LENGTH,
                publicValues.length
            );
        }
        bytes4 magic = bytes4(publicValues[0:4]);
        if (magic != PUBLIC_VALUES_MAGIC) {
            revert InvalidPublicValuesMagic(magic);
        }
        uint16 version = _readUint16(publicValues, 4);
        if (
            version != PUBLIC_VALUES_VERSION &&
            version != PUBLIC_VALUES_V2_VERSION
        ) {
            revert InvalidPublicValuesVersion(version);
        }
        if (
            (version == PUBLIC_VALUES_VERSION &&
                publicValues.length != PUBLIC_VALUES_LENGTH) ||
            (version == PUBLIC_VALUES_V2_VERSION &&
                publicValues.length != PUBLIC_VALUES_V2_LENGTH)
        ) {
            revert InvalidPublicValuesLength(
                version == PUBLIC_VALUES_VERSION
                    ? PUBLIC_VALUES_LENGTH
                    : PUBLIC_VALUES_V2_LENGTH,
                publicValues.length
            );
        }
        decoded.version = version;
        decoded.daMode = uint8(publicValues[6]);
        uint8 reserved = uint8(publicValues[7]);
        if (reserved != 0) revert InvalidReservedByte(reserved);
        decoded.chainId = _readUint64(publicValues, 8);
        decoded.settlementContract = address(bytes20(publicValues[16:36]));
        decoded.batchSequence = _readUint64(publicValues, 36);
        decoded.vkHash = _readBytes32(publicValues, 44);
        decoded.appStatement = _readBytes32(publicValues, 76);
        decoded.minaTransactionHash = _readBytes32(publicValues, 108);

        uint256 cursor = 140;
        for (uint256 i = 0; i < STATE_ARRAY_LENGTH; i++) {
            decoded.stateBefore[i] = _readBytes32(publicValues, cursor);
            cursor += 32;
        }
        for (uint256 i = 0; i < STATE_ARRAY_LENGTH; i++) {
            decoded.stateAfter[i] = _readBytes32(publicValues, cursor);
            cursor += 32;
        }
        decoded.outerActionStateBefore = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.outerActionStateAfter = _readBytes32(publicValues, cursor);
        cursor += 32;
        decoded.outerActionStateLengthBefore = _readUint32(
            publicValues,
            cursor
        );
        cursor += 4;
        decoded.outerActionStateLengthAfter = _readUint32(
            publicValues,
            cursor
        );
        cursor += 4;
        decoded.synchronizedOuterActionState = _readBytes32(
            publicValues,
            cursor
        );
        cursor += 32;
        decoded.synchronizedOuterActionStateLength = _readUint32(
            publicValues,
            cursor
        );
        cursor += 4;
        decoded.slotLower = _readUint32(publicValues, cursor);
        cursor += 4;
        decoded.slotUpper = _readUint32(publicValues, cursor);
        cursor += 4;
        if (version == PUBLIC_VALUES_V2_VERSION) {
            decoded.bridgeAddress = address(
                bytes20(publicValues[cursor:cursor + 20])
            );
            cursor += 20;
            decoded.innerActionRoot = _readBytes32(publicValues, cursor);
            cursor += 32;
            decoded.innerActionStartIndex = _readUint32(
                publicValues,
                cursor
            );
            cursor += 4;
            decoded.innerActionCount = _readUint32(publicValues, cursor);
            cursor += 4;
        }
        assert(cursor == publicValues.length);
    }

    function getDecodedPublicValues(
        bytes calldata publicValues
    ) external pure returns (DecodedPublicValues memory) {
        return decodePublicValues(publicValues);
    }

    function _recordL2ActionState(bytes32 targetActionState) internal {
        if (l2ActionStateInfo[targetActionState].valid) return;
        currentL2ActionStateIndex += 1;
        l2ActionStateInfo[targetActionState] = L2ActionStateInfo({
            index: currentL2ActionStateIndex,
            valid: true
        });
    }

    function _recordAcceptedInnerState(
        bytes32 innerState,
        bytes32 lengthField,
        uint64 settlementIndex
    ) internal {
        if (innerState == bytes32(0)) return;
        acceptedInnerActionState[innerState] = AcceptedInnerActionState({
            settlementIndex: settlementIndex,
            acceptedAt: uint64(block.timestamp),
            length: _fieldToUint32(lengthField),
            valid: true
        });
    }

    function _fieldToUint32(bytes32 value) internal pure returns (uint32) {
        if (uint256(value) > type(uint32).max) {
            revert FieldDoesNotFitUint32(value);
        }
        return uint32(uint256(value));
    }

    function _readBytes32(
        bytes calldata data,
        uint256 offset
    ) private pure returns (bytes32 value) {
        assembly {
            value := calldataload(add(data.offset, offset))
        }
    }

    function _readUint16(
        bytes calldata data,
        uint256 offset
    ) private pure returns (uint16) {
        return uint16(bytes2(data[offset:offset + 2]));
    }

    function _readUint32(
        bytes calldata data,
        uint256 offset
    ) private pure returns (uint32) {
        return uint32(bytes4(data[offset:offset + 4]));
    }

    function _readUint64(
        bytes calldata data,
        uint256 offset
    ) private pure returns (uint64) {
        return uint64(bytes8(data[offset:offset + 8]));
    }

    function _authorizeUpgrade(
        address newImplementation
    ) internal view override onlyRole(UPGRADER_ROLE) {
        newImplementation;
    }
}
