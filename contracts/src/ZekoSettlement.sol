// SPDX-License-Identifier: Not defined
pragma solidity ^0.8.20;

interface ISP1Verifier {
    /// @notice Verifies an SP1 proof.
    /// @param programVKey The verification key for the SP1 program.
    /// @param publicValues The public values committed by the SP1 program.
    /// @param proofBytes The SP1 proof bytes.
    function verifyProof(
        bytes32 programVKey,
        bytes calldata publicValues,
        bytes calldata proofBytes
    ) external view;
}

contract ZekoSettlement {
    uint256 private constant PUBLIC_VALUES_LENGTH = 577;
    uint256 private constant STATE_ARRAY_LENGTH = 8;

    address public owner;
    address public pendingOwner;

    ISP1Verifier public immutable verifier;
    bytes32 public immutable programVKey;

    bytes32 public vkHash;
    bytes32 public actionState;
    bytes32 public currentRoot;
    mapping(bytes32 => bool) public validActionState;

    event OwnershipTransferStarted(
        address indexed previousOwner,
        address indexed newOwner
    );
    event OwnershipTransferred(
        address indexed previousOwner,
        address indexed newOwner
    );
    event OwnershipTransferCanceled(
        address indexed owner,
        address indexed pendingOwner
    );
    event VkHashUpdated(bytes32 indexed oldVkHash, bytes32 indexed newVkHash);
    event ActionStateUpdated(
        bytes32 indexed oldActionState,
        bytes32 indexed newActionState
    );
    event RootUpdated(bytes32 indexed oldRoot, bytes32 indexed newRoot);

    error NotOwner();
    error NotPendingOwner();
    error ZeroAddress();
    error InvalidPublicValuesLength(uint256 expected, uint256 actual);
    error InvalidBool(uint8 value);
    error InvalidProofFlag();
    error InvalidVkHash(bytes32 expected, bytes32 actual);
    error InvalidActionState(bytes32 expected, bytes32 actual);
    error InvalidCurrentRoot(bytes32 expected, bytes32 actual);

    struct DecodedPublicValues {
        bool proofValid;
        bytes32 vkHash;
        bytes32[8] stateBefore;
        bytes32[8] stateAfter;
        bytes32 actionStateBefore;
    }

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    modifier onlyPendingOwner() {
        if (msg.sender != pendingOwner) revert NotPendingOwner();
        _;
    }

    constructor(
        address verifier_,
        bytes32 programVKey_,
        bytes32 initialVkHash_,
        bytes32 initialActionState_,
        bytes32 initialRoot_
    ) {
        if (verifier_ == address(0)) revert ZeroAddress();

        owner = msg.sender;
        verifier = ISP1Verifier(verifier_);
        programVKey = programVKey_;

        vkHash = initialVkHash_;
        actionState = initialActionState_;
        currentRoot = initialRoot_;
        validActionState[initialActionState_] = true;

        emit OwnershipTransferred(address(0), msg.sender);
        emit VkHashUpdated(bytes32(0), initialVkHash_);
        emit ActionStateUpdated(bytes32(0), initialActionState_);
        emit RootUpdated(bytes32(0), initialRoot_);
    }

    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();

        pendingOwner = newOwner;

        emit OwnershipTransferStarted(owner, newOwner);
    }

    function acceptOwnership() external onlyPendingOwner {
        address oldOwner = owner;
        address newOwner = pendingOwner;

        owner = newOwner;
        pendingOwner = address(0);

        emit OwnershipTransferred(oldOwner, newOwner);
    }

    function cancelOwnershipTransfer() external onlyOwner {
        address oldPendingOwner = pendingOwner;

        pendingOwner = address(0);

        emit OwnershipTransferCanceled(owner, oldPendingOwner);
    }

    function setVkHash(bytes32 newVkHash) external onlyOwner {
        bytes32 oldVkHash = vkHash;
        vkHash = newVkHash;

        emit VkHashUpdated(oldVkHash, newVkHash);
    }

    function setActionState(bytes32 newActionState) external onlyOwner {
        bytes32 oldActionState = actionState;
        actionState = newActionState;
        validActionState[newActionState] = true;

        emit ActionStateUpdated(oldActionState, newActionState);
    }

    function isActionStateValid(
        bytes32 targetActionState
    ) external view returns (bool) {
        return validActionState[targetActionState];
    }

    function verifyAndUpdateRoot(
        bytes calldata publicValues,
        bytes calldata proofBytes
    ) external {
        verifier.verifyProof(programVKey, publicValues, proofBytes);

        DecodedPublicValues memory decoded = decodePublicValues(publicValues);

        if (!decoded.proofValid) {
            revert InvalidProofFlag();
        }

        if (decoded.vkHash != vkHash) {
            revert InvalidVkHash(vkHash, decoded.vkHash);
        }

        if (decoded.actionStateBefore != actionState) {
            revert InvalidActionState(actionState, decoded.actionStateBefore);
        }

        validActionState[decoded.actionStateBefore] = true;

        if (decoded.stateBefore[3] != currentRoot) {
            revert InvalidCurrentRoot(currentRoot, decoded.stateBefore[3]);
        }

        bytes32 oldRoot = currentRoot;
        bytes32 newRoot = decoded.stateAfter[3];

        currentRoot = newRoot;

        emit RootUpdated(oldRoot, newRoot);
    }

    function decodePublicValues(
        bytes calldata publicValues
    ) public pure returns (DecodedPublicValues memory decoded) {
        if (publicValues.length != PUBLIC_VALUES_LENGTH) {
            revert InvalidPublicValuesLength(
                PUBLIC_VALUES_LENGTH,
                publicValues.length
            );
        }

        uint256 cursor = 0;

        decoded.proofValid = _readBool(publicValues, cursor);
        cursor += 1;

        decoded.vkHash = _readBytes32(publicValues, cursor);
        cursor += 32;

        for (uint256 i = 0; i < STATE_ARRAY_LENGTH; i++) {
            decoded.stateBefore[i] = _readBytes32(publicValues, cursor);
            cursor += 32;
        }

        for (uint256 i = 0; i < STATE_ARRAY_LENGTH; i++) {
            decoded.stateAfter[i] = _readBytes32(publicValues, cursor);
            cursor += 32;
        }

        decoded.actionStateBefore = _readBytes32(publicValues, cursor);
        cursor += 32;

        assert(cursor == PUBLIC_VALUES_LENGTH);
    }

    function getDecodedPublicValues(
        bytes calldata publicValues
    ) external pure returns (DecodedPublicValues memory decoded) {
        return decodePublicValues(publicValues);
    }

    function _readBool(
        bytes calldata data,
        uint256 offset
    ) private pure returns (bool value) {
        uint8 raw = uint8(data[offset]);

        if (raw > 1) {
            revert InvalidBool(raw);
        }

        return raw == 1;
    }

    function _readBytes32(
        bytes calldata data,
        uint256 offset
    ) private pure returns (bytes32 value) {
        assembly {
            value := calldataload(add(data.offset, offset))
        }
    }
}
