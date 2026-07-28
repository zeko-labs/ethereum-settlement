// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {IERC20Metadata} from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Metadata.sol";

enum AssetStatus {
    None,
    Pending,
    Active,
    Paused,
    Disallowed
}

/// @notice Immutable cross-chain identity. Field order is the V1 ABI wire.
struct AssetRecord {
    uint32 schemaVersion;
    uint32 registryIndex;
    bytes32 assetId;
    address ethereumToken;
    bytes32 tokenOwnerL2;
    bytes32 tokenIdL2;
    uint8 decimals;
    uint64 inventoryCap;
    bytes32 mftStandardVkId;
    bytes32 vaultPublicKey;
    bytes32 universalBridgeVkId;
}

interface IZekoAssetRegistry {
    error InvalidAssetRecord();
    error AssetRecordAlreadyProposed(bytes32 recordHash);
    error AssetIdentityAlreadyProposed(bytes32 identity);
    error AssetRecordNotPending(bytes32 recordHash);
    error AssetRecordNotSettled(bytes32 recordHash);
    error InvalidAssetRecordBatch(bytes32 expectedRoot, bytes32 actualRoot, uint64 settlementSequence);
    error RegistryCheckpointMismatch(
        bytes32 expectedRoot,
        uint32 expectedCount,
        uint32 expectedSchema,
        bytes32 actualRoot,
        uint32 actualCount,
        uint32 actualSchema
    );
    error TokenAlreadyAdded(address token);
    error InvalidEthereumDecimals(address token, uint8 expected, uint8 actual);
    error UnauthorizedAssetRegistryCaller(address caller);

    event AssetRegistrationProposed(
        bytes32 indexed recordHash,
        uint32 indexed registryIndex,
        address indexed token,
        bytes32 assetId,
        bytes32 tokenOwnerL2,
        bytes32 tokenIdL2,
        uint8 decimals,
        uint64 inventoryCap,
        bytes32 mftStandardVkId,
        bytes32 vaultPublicKey,
        bytes32 universalBridgeVkId
    );

    event AssetRegistrationActivated(
        bytes32 indexed recordHash,
        uint32 indexed registryIndex,
        address indexed token,
        bytes32 recordCommitment,
        bytes32 registryRoot,
        uint32 registryCount,
        uint32 registrySchemaVersion
    );

    event AssetStatusUpdated(address indexed token, AssetStatus oldStatus, AssetStatus newStatus);

    function proposeAsset(AssetRecord calldata record) external returns (bytes32 recordHash);

    function activateAsset(address token, bytes32 expectedRegistryRoot, uint32 expectedRegistryCount) external;

    function activateAssetFromBatch(
        address token,
        uint64 settlementSequence,
        bytes32 recordCommitment,
        bytes32[8] calldata siblings
    ) external;

    function setAssetStatus(address token, AssetStatus newStatus) external;

    function assetRecord(address token) external view returns (AssetRecord memory);

    function assetTokenByRegistryIndex(uint32 registryIndex) external view returns (address);

    function assetStatusByToken(address token) external view returns (AssetStatus);

    function proposedAssetRecord(bytes32 recordHash) external view returns (bool);

    function proposedAssetId(bytes32 assetId) external view returns (bool);

    function proposedL2TokenIdentity(bytes32 identity) external view returns (bool);

    function proposedAssetCount() external view returns (uint32);

    function hashAssetRecord(AssetRecord memory record) external pure returns (bytes32);
}

interface IZekoAssetRegistryHost {
    function ADMIN_ROLE() external view returns (bytes32);

    function hasRole(bytes32 role, address account) external view returns (bool);

    function canonicalTokenRegistered(address token) external view returns (bool);

    function allowedToken(address token)
        external
        view
        returns (uint8 zekoDecimals, uint8 ethereumDecimals, bool allowed);

    function settlementVerifier() external view returns (address);

    function activateAssetRecordFromRegistry(AssetRecord calldata record, bytes32 recordCommitment) external;

    function setAssetAllowedFromRegistry(address token, bool allowed) external;
}

interface IZekoAssetRegistrySettlement {
    function assetRegistryRoot() external view returns (bytes32);

    function assetRegistryCount() external view returns (uint32);

    function assetRegistrySchemaVersion() external view returns (uint32);

    function settledAssetRecord(bytes32 recordHash) external view returns (bool);

    function settledAssetRecordCommitment(bytes32 recordHash) external view returns (bytes32);

    function assetRegistryRecordBatch(uint64 sequence)
        external
        view
        returns (
            bytes32 registryRoot,
            uint32 registryCount,
            uint32 registrySchemaVersion,
            bytes32 recordBatchRoot,
            uint32 recordBatchCount,
            bool valid
        );
}

library ZekoAssetRegistryStorage {
    bytes32 internal constant SLOT = keccak256("zeko.ethereum.bridge.asset.registry.storage.v1");

    struct Layout {
        mapping(address => AssetRecord) assetRecordByToken;
        mapping(uint32 => address) assetTokenByRegistryIndex;
        mapping(address => AssetStatus) assetStatusByToken;
        mapping(bytes32 => bool) proposedAssetRecord;
        mapping(bytes32 => bool) proposedAssetId;
        mapping(bytes32 => bool) proposedL2TokenIdentity;
        uint32 proposedAssetCount;
    }

    function registryStorage() internal pure returns (Layout storage value) {
        bytes32 slot = SLOT;
        assembly ("memory-safe") {
            value.slot := slot
        }
    }
}

/// @notice Registry facet executed in the bridge proxy context.
/// @dev The bridge fallback delegates registry selectors here. Registry state
/// is namespaced, while custody configuration is applied through self-only
/// callbacks on the bridge implementation.
contract ZekoAssetRegistry is IZekoAssetRegistry {
    bytes32 public constant ERC20_ASSET_V1_DOMAIN = keccak256("ZEKO_ERC20_ASSET_V1");
    bytes32 public constant ERC20_ASSET_RECORD_V1_DOMAIN = keccak256("ZEKO_ERC20_ASSET_RECORD_V1");
    bytes32 public constant ASSET_RECORD_BATCH_LEAF_V2_DOMAIN = keccak256("ZEKO_ASSET_RECORD_BATCH_LEAF_V2");
    bytes32 public constant ASSET_RECORD_BATCH_NODE_V1_DOMAIN = keccak256("ZEKO_ASSET_RECORD_BATCH_NODE_V1");

    uint256 public constant ASSET_REGISTRY_TREE_DEPTH = 8;
    uint256 public constant ASSET_REGISTRY_CAPACITY = 2 ** ASSET_REGISTRY_TREE_DEPTH;
    uint32 public constant ERC20_ASSET_RECORD_SCHEMA_V1 = 1;
    uint8 private constant MAX_ZEKO_DECIMALS = 9;

    modifier onlyBridgeAdmin() {
        IZekoAssetRegistryHost host = IZekoAssetRegistryHost(address(this));
        if (!host.hasRole(host.ADMIN_ROLE(), msg.sender)) {
            revert UnauthorizedAssetRegistryCaller(msg.sender);
        }
        _;
    }

    function proposeAsset(AssetRecord calldata record) external onlyBridgeAdmin returns (bytes32 recordHash) {
        ZekoAssetRegistryStorage.Layout storage registry = ZekoAssetRegistryStorage.registryStorage();
        if (
            record.schemaVersion != ERC20_ASSET_RECORD_SCHEMA_V1 || record.registryIndex != registry.proposedAssetCount
                || record.registryIndex >= ASSET_REGISTRY_CAPACITY || record.ethereumToken == address(0)
                || record.assetId == bytes32(0) || record.tokenOwnerL2 == bytes32(0) || record.tokenIdL2 == bytes32(0)
                || record.decimals > MAX_ZEKO_DECIMALS || record.inventoryCap == 0
                || record.mftStandardVkId == bytes32(0) || record.vaultPublicKey == bytes32(0)
                || record.universalBridgeVkId == bytes32(0) || record.tokenOwnerL2 == record.vaultPublicKey
        ) revert InvalidAssetRecord();

        IZekoAssetRegistryHost host = IZekoAssetRegistryHost(address(this));
        (,, bool alreadyAllowed) = host.allowedToken(record.ethereumToken);
        if (
            host.canonicalTokenRegistered(record.ethereumToken)
                || registry.assetStatusByToken[record.ethereumToken] != AssetStatus.None || alreadyAllowed
        ) revert TokenAlreadyAdded(record.ethereumToken);

        uint8 actualDecimals = IERC20Metadata(record.ethereumToken).decimals();
        if (actualDecimals != record.decimals) {
            revert InvalidEthereumDecimals(record.ethereumToken, record.decimals, actualDecimals);
        }
        if (
            record.assetId
                != _computeERC20AssetId(record.ethereumToken, record.tokenOwnerL2, record.tokenIdL2, record.decimals)
        ) revert InvalidAssetRecord();

        recordHash = _hashAssetRecord(record);
        if (registry.proposedAssetRecord[recordHash]) {
            revert AssetRecordAlreadyProposed(recordHash);
        }
        if (registry.proposedAssetId[record.assetId]) {
            revert AssetIdentityAlreadyProposed(record.assetId);
        }
        bytes32 l2Identity = keccak256(abi.encode(record.tokenOwnerL2, record.tokenIdL2));
        if (registry.proposedL2TokenIdentity[l2Identity]) {
            revert AssetIdentityAlreadyProposed(l2Identity);
        }

        registry.assetRecordByToken[record.ethereumToken] = record;
        registry.assetTokenByRegistryIndex[record.registryIndex] = record.ethereumToken;
        registry.assetStatusByToken[record.ethereumToken] = AssetStatus.Pending;
        registry.proposedAssetRecord[recordHash] = true;
        registry.proposedAssetId[record.assetId] = true;
        registry.proposedL2TokenIdentity[l2Identity] = true;
        registry.proposedAssetCount = record.registryIndex + 1;

        emit AssetRegistrationProposed(
            recordHash,
            record.registryIndex,
            record.ethereumToken,
            record.assetId,
            record.tokenOwnerL2,
            record.tokenIdL2,
            record.decimals,
            record.inventoryCap,
            record.mftStandardVkId,
            record.vaultPublicKey,
            record.universalBridgeVkId
        );
    }

    function activateAsset(address token, bytes32 expectedRegistryRoot, uint32 expectedRegistryCount)
        external
        onlyBridgeAdmin
    {
        (AssetRecord memory record, bytes32 recordHash) = _pendingRecord(token);
        IZekoAssetRegistrySettlement settlement = _settlement();
        if (!settlement.settledAssetRecord(recordHash)) {
            revert AssetRecordNotSettled(recordHash);
        }
        bytes32 recordCommitment = settlement.settledAssetRecordCommitment(recordHash);
        if (recordCommitment == bytes32(0)) {
            revert AssetRecordNotSettled(recordHash);
        }

        bytes32 actualRoot = settlement.assetRegistryRoot();
        uint32 actualCount = settlement.assetRegistryCount();
        uint32 actualSchema = settlement.assetRegistrySchemaVersion();
        if (
            actualRoot != expectedRegistryRoot || actualCount != expectedRegistryCount
                || actualSchema != record.schemaVersion || actualCount <= record.registryIndex
        ) {
            revert RegistryCheckpointMismatch(
                expectedRegistryRoot, expectedRegistryCount, record.schemaVersion, actualRoot, actualCount, actualSchema
            );
        }
        _activate(record, recordHash, recordCommitment, actualRoot, actualCount, actualSchema);
    }

    function activateAssetFromBatch(
        address token,
        uint64 settlementSequence,
        bytes32 recordCommitment,
        bytes32[8] calldata siblings
    ) external onlyBridgeAdmin {
        (AssetRecord memory record, bytes32 recordHash) = _pendingRecord(token);
        (
            bytes32 registryRoot,
            uint32 registryCount,
            uint32 registrySchema,
            bytes32 recordBatchRoot,
            uint32 recordBatchCount,
            bool valid
        ) = _settlement().assetRegistryRecordBatch(settlementSequence);
        if (
            !valid || recordBatchRoot == bytes32(0) || recordBatchCount == 0 || recordCommitment == bytes32(0)
                || registrySchema != record.schemaVersion || record.registryIndex >= registryCount
        ) {
            revert RegistryCheckpointMismatch(
                registryRoot, registryCount, record.schemaVersion, registryRoot, registryCount, registrySchema
            );
        }

        bytes32 computed = keccak256(abi.encodePacked(ASSET_RECORD_BATCH_LEAF_V2_DOMAIN, recordHash, recordCommitment));
        uint256 index = record.registryIndex;
        for (uint256 level = 0; level < ASSET_REGISTRY_TREE_DEPTH; level++) {
            computed = (index & 1) == 0
                ? keccak256(abi.encodePacked(ASSET_RECORD_BATCH_NODE_V1_DOMAIN, computed, siblings[level]))
                : keccak256(abi.encodePacked(ASSET_RECORD_BATCH_NODE_V1_DOMAIN, siblings[level], computed));
            index >>= 1;
        }
        if (computed != recordBatchRoot) {
            revert InvalidAssetRecordBatch(recordBatchRoot, computed, settlementSequence);
        }
        _activate(record, recordHash, recordCommitment, registryRoot, registryCount, registrySchema);
    }

    function setAssetStatus(address token, AssetStatus newStatus) external onlyBridgeAdmin {
        ZekoAssetRegistryStorage.Layout storage registry = ZekoAssetRegistryStorage.registryStorage();
        AssetStatus oldStatus = registry.assetStatusByToken[token];
        if (
            oldStatus == AssetStatus.None || oldStatus == AssetStatus.Pending || newStatus == AssetStatus.None
                || newStatus == AssetStatus.Pending
        ) revert InvalidAssetRecord();
        registry.assetStatusByToken[token] = newStatus;
        IZekoAssetRegistryHost(address(this)).setAssetAllowedFromRegistry(token, newStatus == AssetStatus.Active);
        emit AssetStatusUpdated(token, oldStatus, newStatus);
    }

    function assetRecord(address token) external view returns (AssetRecord memory) {
        return ZekoAssetRegistryStorage.registryStorage().assetRecordByToken[token];
    }

    function assetTokenByRegistryIndex(uint32 registryIndex) external view returns (address) {
        return ZekoAssetRegistryStorage.registryStorage().assetTokenByRegistryIndex[registryIndex];
    }

    function assetStatusByToken(address token) external view returns (AssetStatus) {
        return ZekoAssetRegistryStorage.registryStorage().assetStatusByToken[token];
    }

    function proposedAssetRecord(bytes32 recordHash) external view returns (bool) {
        return ZekoAssetRegistryStorage.registryStorage().proposedAssetRecord[recordHash];
    }

    function proposedAssetId(bytes32 assetId) external view returns (bool) {
        return ZekoAssetRegistryStorage.registryStorage().proposedAssetId[assetId];
    }

    function proposedL2TokenIdentity(bytes32 identity) external view returns (bool) {
        return ZekoAssetRegistryStorage.registryStorage().proposedL2TokenIdentity[identity];
    }

    function proposedAssetCount() external view returns (uint32) {
        return ZekoAssetRegistryStorage.registryStorage().proposedAssetCount;
    }

    function hashAssetRecord(AssetRecord memory record) external pure returns (bytes32) {
        return _hashAssetRecord(record);
    }

    function _pendingRecord(address token) private view returns (AssetRecord memory record, bytes32 recordHash) {
        ZekoAssetRegistryStorage.Layout storage registry = ZekoAssetRegistryStorage.registryStorage();
        record = registry.assetRecordByToken[token];
        recordHash = _hashAssetRecord(record);
        if (registry.assetStatusByToken[token] != AssetStatus.Pending) {
            revert AssetRecordNotPending(recordHash);
        }
    }

    function _activate(
        AssetRecord memory record,
        bytes32 recordHash,
        bytes32 recordCommitment,
        bytes32 registryRoot,
        uint32 registryCount,
        uint32 registrySchema
    ) private {
        ZekoAssetRegistryStorage.registryStorage().assetStatusByToken[record.ethereumToken] = AssetStatus.Active;
        IZekoAssetRegistryHost(address(this)).activateAssetRecordFromRegistry(record, recordCommitment);
        emit AssetRegistrationActivated(
            recordHash,
            record.registryIndex,
            record.ethereumToken,
            recordCommitment,
            registryRoot,
            registryCount,
            registrySchema
        );
    }

    function _settlement() private view returns (IZekoAssetRegistrySettlement) {
        return IZekoAssetRegistrySettlement(IZekoAssetRegistryHost(address(this)).settlementVerifier());
    }

    function _hashAssetRecord(AssetRecord memory record) private pure returns (bytes32) {
        return keccak256(
            abi.encode(
                ERC20_ASSET_RECORD_V1_DOMAIN,
                record.schemaVersion,
                record.registryIndex,
                record.assetId,
                record.ethereumToken,
                record.tokenOwnerL2,
                record.tokenIdL2,
                record.decimals,
                record.inventoryCap,
                record.mftStandardVkId,
                record.vaultPublicKey,
                record.universalBridgeVkId
            )
        );
    }

    function _computeERC20AssetId(address token, bytes32 zekoTokenOwner, bytes32 zekoTokenId, uint8 decimals)
        private
        view
        returns (bytes32)
    {
        return keccak256(
            abi.encode(
                ERC20_ASSET_V1_DOMAIN, block.chainid, address(this), token, zekoTokenOwner, zekoTokenId, decimals
            )
        );
    }
}
