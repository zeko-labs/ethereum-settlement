// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {Vm} from "forge-std/Vm.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {IAccessControl} from "@openzeppelin/contracts/access/IAccessControl.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

import {EthereumZekoBridge} from "../src/EthereumZekoBridge.sol";
import {AssetRecord, AssetStatus, IZekoAssetRegistry, ZekoAssetRegistry} from "../src/ZekoAssetRegistry.sol";
import {ZekoAddress, ZekoAddressLib} from "../src/ZekoAddress.sol";
import {ISP1Verifier} from "../src/ZekoSettlement.sol";

contract TestERC20 is ERC20 {
    uint8 private immutable _decimals;

    constructor(string memory name_, string memory symbol_, uint8 decimals_) ERC20(name_, symbol_) {
        _decimals = decimals_;
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }

    function decimals() public view override returns (uint8) {
        return _decimals;
    }
}

contract MockSettlementVerifier {
    mapping(bytes32 => bool) public validActionState;
    mapping(bytes32 => uint64) public l2ActionStateIndex;
    bytes32 public actionState;
    uint32 public outerActionStateLength;
    uint32 public appendCalls;
    uint64 public virtualSlot;
    bytes32 public assetRegistryRoot;
    uint32 public assetRegistryCount;
    uint32 public assetRegistrySchemaVersion;
    mapping(bytes32 => bool) public settledAssetRecord;
    mapping(bytes32 => bytes32) public settledAssetRecordCommitment;

    struct AssetRecordBatch {
        bytes32 registryRoot;
        uint32 registryCount;
        uint32 registrySchemaVersion;
        bytes32 recordBatchRoot;
        uint32 recordBatchCount;
        bool valid;
    }

    struct Batch {
        bytes32 minaStateBefore;
        bytes32 minaStateAfter;
        bytes32 root;
        uint32 startIndex;
        uint32 count;
        uint32 commitSlotUpper;
        bool valid;
    }

    mapping(uint64 => Batch) private _batches;
    mapping(uint64 => AssetRecordBatch) public assetRegistryRecordBatch;

    function setCurrentActionState(bytes32 value) external {
        actionState = value;
    }

    function setOuterActionStateLength(uint32 value) external {
        outerActionStateLength = value;
    }

    function appendOuterWitnessBatch(bytes32 stateBefore, bytes32 stateAfter, uint32 count) external {
        require(stateBefore == actionState, "stale action state");
        actionState = stateAfter;
        outerActionStateLength += count;
        appendCalls += 1;
    }

    function setVirtualSlot(uint64 value) external {
        virtualSlot = value;
    }

    function setAssetRegistryCheckpoint(
        bytes32 root,
        uint32 count,
        uint32 schemaVersion,
        bytes32 recordHash,
        bytes32 recordCommitment
    ) external {
        assetRegistryRoot = root;
        assetRegistryCount = count;
        assetRegistrySchemaVersion = schemaVersion;
        settledAssetRecord[recordHash] = true;
        settledAssetRecordCommitment[recordHash] = recordCommitment;
    }

    function setAssetRegistryRecordBatch(
        uint64 sequence,
        bytes32 registryRoot,
        uint32 registryCount,
        uint32 registrySchemaVersion,
        bytes32 recordBatchRoot,
        uint32 recordBatchCount
    ) external {
        assetRegistryRoot = registryRoot;
        assetRegistryCount = registryCount;
        assetRegistrySchemaVersion = registrySchemaVersion;
        assetRegistryRecordBatch[sequence] = AssetRecordBatch({
            registryRoot: registryRoot,
            registryCount: registryCount,
            registrySchemaVersion: registrySchemaVersion,
            recordBatchRoot: recordBatchRoot,
            recordBatchCount: recordBatchCount,
            valid: true
        });
    }

    function currentVirtualSlot() external view returns (uint64) {
        return virtualSlot;
    }

    function setInnerActionBatch(uint64 sequence, bytes32 root, uint32 startIndex, uint32 count, uint32 commitSlotUpper)
        external
    {
        _batches[sequence] = Batch({
            minaStateBefore: keccak256("inner before"),
            minaStateAfter: keccak256("inner after"),
            root: root,
            startIndex: startIndex,
            count: count,
            commitSlotUpper: commitSlotUpper,
            valid: true
        });
    }

    function innerActionBatch(uint64 sequence)
        external
        view
        returns (bytes32, bytes32, bytes32, uint32, uint32, uint32, bool)
    {
        Batch memory batch = _batches[sequence];
        return (
            batch.minaStateBefore,
            batch.minaStateAfter,
            batch.root,
            batch.startIndex,
            batch.count,
            batch.commitSlotUpper,
            batch.valid
        );
    }

    function setActionStateValid(bytes32 targetActionState, bool valid) external {
        validActionState[targetActionState] = valid;
    }

    function setL2ActionStateInfo(bytes32 targetActionState, uint64 index, bool valid) external {
        l2ActionStateIndex[targetActionState] = index;
        validActionState[targetActionState] = valid;
    }

    function isActionStateValid(bytes32 targetActionState) external view returns (bool) {
        return validActionState[targetActionState];
    }

    function l2ActionStateInfo(bytes32 targetActionState) external view returns (uint64 index, bool valid) {
        return (l2ActionStateIndex[targetActionState], validActionState[targetActionState]);
    }
}

contract MockSP1Verifier is ISP1Verifier {
    bool public shouldRevert;
    bytes32 public lastProgramVKey;
    bytes public lastPublicValues;
    bytes public lastProofBytes;

    function setShouldRevert(bool value) external {
        shouldRevert = value;
    }

    function verifyProof(bytes32 programVKey, bytes calldata publicValues, bytes calldata proofBytes)
        external
        view
        override
    {
        programVKey;
        publicValues;
        proofBytes;
        if (shouldRevert) revert("invalid proof");
    }
}

contract EthereumZekoBridgeTest is Test {
    uint256 private constant ZEKO_FIELD_ORDER =
        28948022309329048855892746252171976963363056481941560715954676764349967630337;

    EthereumZekoBridge internal bridge;
    IZekoAssetRegistry internal registry;
    ZekoAssetRegistry internal registryModule;
    MockSettlementVerifier internal settlement;
    MockSP1Verifier internal sp1Verifier;
    TestERC20 internal token18;
    TestERC20 internal token6;
    TestERC20 internal token9;
    bytes32 internal bridgeProgramVKey = keccak256("bridge program vkey");
    EthereumZekoBridge internal implementation;

    address internal owner = address(this);
    address internal alice = address(0xA11CE);
    address internal bob = address(0xB0B);

    function setUp() public {
        settlement = new MockSettlementVerifier();
        sp1Verifier = new MockSP1Verifier();
        registryModule = new ZekoAssetRegistry();
        implementation = new EthereumZekoBridge(registryModule);
        ERC1967Proxy proxy = new ERC1967Proxy(
            address(implementation),
            abi.encodeCall(
                EthereumZekoBridge.initialize, (owner, address(settlement), address(sp1Verifier), bridgeProgramVKey)
            )
        );
        bridge = EthereumZekoBridge(payable(address(proxy)));
        registry = IZekoAssetRegistry(address(proxy));
        token18 = new TestERC20("Token18", "TK18", 18);
        token6 = new TestERC20("Token6", "TK6", 6);
        token9 = new TestERC20("Token9", "TK9", 9);

        token18.mint(alice, 100 ether);
        token6.mint(alice, 100 * 10 ** 6);
        token9.mint(alice, 100 * 10 ** 9);
    }

    function _activateCanonicalToken(TestERC20 token, bytes32 ownerL2, bytes32 tokenIdL2, uint64 inventoryCap)
        private
        returns (AssetRecord memory record, bytes32 recordCommitment)
    {
        uint32 registryIndex = registry.proposedAssetCount();
        uint8 decimals = token.decimals();
        record = AssetRecord({
            schemaVersion: 1,
            registryIndex: registryIndex,
            assetId: bridge.computeERC20AssetId(address(token), ownerL2, tokenIdL2, decimals),
            ethereumToken: address(token),
            tokenOwnerL2: ownerL2,
            tokenIdL2: tokenIdL2,
            decimals: decimals,
            inventoryCap: inventoryCap,
            mftStandardVkId: keccak256(abi.encode("test mft standard vk", registryIndex)),
            vaultPublicKey: bytes32(uint256(0xA00000) + registryIndex),
            universalBridgeVkId: keccak256(abi.encode("test universal bridge vk", registryIndex))
        });
        bytes32 recordHash = registry.proposeAsset(record);
        recordCommitment = bytes32(uint256(1_000_000) + registryIndex);
        bytes32 settledRoot = keccak256(abi.encode("settled registry root", registryIndex, recordHash));
        settlement.setAssetRegistryCheckpoint(settledRoot, registryIndex + 1, 1, recordHash, recordCommitment);
        registry.activateAsset(address(token), settledRoot, registryIndex + 1);
    }

    function test_SetUp_ConfiguresNativeETH() public view {
        (uint8 zekoDecimals, uint8 ethereumDecimals, bool allowed) = bridge.allowedToken(address(0));

        assertEq(zekoDecimals, 9);
        assertEq(ethereumDecimals, 18);
        assertTrue(allowed);
        assertEq(address(bridge.settlementVerifier()), address(settlement));
        assertEq(address(bridge.bridgeVerifier()), address(sp1Verifier));
        assertEq(bridge.bridgeProgramVKey(), bridgeProgramVKey);
        assertTrue(bridge.hasRole(bridge.DEFAULT_ADMIN_ROLE(), owner));
        assertTrue(bridge.hasRole(bridge.ADMIN_ROLE(), owner));
        assertTrue(bridge.hasRole(bridge.PROVER_ROLE(), owner));
        assertTrue(bridge.hasRole(bridge.UPGRADER_ROLE(), owner));
    }

    function test_RuntimeCodeFitsEIP170WithHeadroom() public view {
        assertLe(address(implementation).code.length, 22_000);
    }

    function test_Upgrade_RevertsWhenNotUpgrader() public {
        EthereumZekoBridge newImplementation = new EthereumZekoBridge(registryModule);
        bytes32 upgraderRole = bridge.UPGRADER_ROLE();

        vm.expectRevert(
            abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, alice, upgraderRole)
        );
        vm.prank(alice);
        bridge.upgradeToAndCall(address(newImplementation), "");
    }

    function test_Upgrade_AllowsUpgrader() public {
        EthereumZekoBridge newImplementation = new EthereumZekoBridge(registryModule);

        bridge.upgradeToAndCall(address(newImplementation), "");

        assertEq(bridge.currentDepositState(), bridge.INITIAL_DEPOSIT_STATE());
    }

    function test_SubmitDepositRecordsCanonicalERC20WitnessInput() public {
        bytes32 zekoTokenOwner = bytes32(uint256(0x123456));
        bytes32 zekoTokenId = keccak256("zeko fungible token id");
        (, bytes32 recordCommitment) = _activateCanonicalToken(token9, zekoTokenOwner, zekoTokenId, type(uint64).max);
        uint64 zekoAmount = 2 * 10 ** 9;
        uint256 amount = zekoAmount;
        ZekoAddress recipient = ZekoAddressLib.pack(0x01020304, false);

        vm.startPrank(alice);
        token9.approve(address(bridge), amount);
        vm.recordLogs();
        (uint64 nonce, bytes32 leaf, bytes32 newState) = bridge.submitDeposit(address(token9), amount, recipient);
        Vm.Log[] memory logs = vm.getRecordedLogs();
        vm.stopPrank();

        bytes32 bridgeDepositSignature =
            keccak256("BridgeDeposit(uint64,bytes32,bytes32,bytes32,address,address,uint256,uint256,uint256,uint64)");
        bytes32 erc20DepositSignature =
            keccak256("ERC20DepositSubmitted(uint64,bytes32,bytes32,bytes32,address,address,uint256,uint64,uint64)");
        bytes32 erc20DepositV2Signature = keccak256(
            "ERC20DepositSubmittedV2(uint64,bytes32,bytes32,bytes32,address,address,uint256,uint64,uint64,uint32,uint32,bytes32)"
        );
        bool foundWitnessEvent;
        bool foundCanonicalEvent;
        bool foundRegistryBoundEvent;
        for (uint256 i = 0; i < logs.length; i++) {
            if (logs[i].topics[0] == bridgeDepositSignature) {
                foundWitnessEvent = true;
                assertEq(uint64(uint256(logs[i].topics[1])), nonce);
                assertEq(logs[i].topics[2], leaf);
                assertEq(logs[i].topics[3], newState);
                (
                    ,
                    address eventToken,
                    address eventSender,
                    uint256 eventRecipient,
                    uint256 eventAmount,
                    uint256 eventZekoAmount,
                    uint64 eventTimeout
                ) = abi.decode(logs[i].data, (bytes32, address, address, uint256, uint256, uint256, uint64));
                assertEq(eventToken, address(token9));
                assertEq(eventSender, alice);
                assertEq(eventRecipient, ZekoAddress.unwrap(recipient));
                assertEq(eventAmount, amount);
                assertEq(eventZekoAmount, amount);
                assertEq(eventTimeout, type(uint32).max);
            } else if (logs[i].topics[0] == erc20DepositSignature) {
                foundCanonicalEvent = true;
                assertEq(uint64(uint256(logs[i].topics[1])), nonce);
                assertEq(logs[i].topics[2], bridge.assetIdByToken(address(token9)));
                assertEq(logs[i].topics[3], leaf);
                (
                    bytes32 eventState,
                    address eventToken,
                    address eventSender,
                    uint256 eventRecipient,
                    uint64 eventAmount,
                    uint64 eventTimeout
                ) = abi.decode(logs[i].data, (bytes32, address, address, uint256, uint64, uint64));
                assertEq(eventState, newState);
                assertEq(eventToken, address(token9));
                assertEq(eventSender, alice);
                assertEq(eventRecipient, ZekoAddress.unwrap(recipient));
                assertEq(eventAmount, amount);
                assertEq(eventTimeout, type(uint32).max);
            } else if (logs[i].topics[0] == erc20DepositV2Signature) {
                foundRegistryBoundEvent = true;
                (,,,,,, uint32 encodingVersion, uint32 registryIndex, bytes32 eventRecordCommitment) = abi.decode(
                    logs[i].data, (bytes32, address, address, uint256, uint64, uint64, uint32, uint32, bytes32)
                );
                assertEq(encodingVersion, 2);
                assertEq(registryIndex, 0);
                assertEq(eventRecordCommitment, recordCommitment);
            }
        }
        assertTrue(foundWitnessEvent);
        assertTrue(foundCanonicalEvent);
        assertTrue(foundRegistryBoundEvent);

        assertEq(nonce, 1);
        assertEq(
            leaf,
            bridge.computeERC20DepositLeaf(
                address(token9),
                0,
                recordCommitment,
                bridge.assetIdByToken(address(token9)),
                recipient,
                zekoAmount,
                type(uint32).max,
                nonce
            )
        );
        assertEq(newState, bridge.currentDepositState());
        assertEq(token9.balanceOf(address(bridge)), amount);
        assertEq(bridge.escrowLiabilityByToken(address(token9)), amount);
        assertEq(bridge.zekoTokenOwnerByToken(address(token9)), zekoTokenOwner);
        assertEq(bridge.zekoTokenIdByToken(address(token9)), zekoTokenId);
        assertEq(bridge.depositCapByToken(address(token9)), type(uint64).max);
        assertEq(
            bridge.assetIdByToken(address(token9)),
            bridge.computeERC20AssetId(address(token9), zekoTokenOwner, zekoTokenId, 9)
        );
    }

    function test_UniversalAssetRemainsPendingUntilMatchingSettlement() public {
        bytes32 ownerL2 = bytes32(uint256(0x123456));
        bytes32 tokenIdL2 = keccak256("universal token id");
        AssetRecord memory record = AssetRecord({
            schemaVersion: 1,
            registryIndex: 0,
            assetId: bridge.computeERC20AssetId(address(token6), ownerL2, tokenIdL2, 6),
            ethereumToken: address(token6),
            tokenOwnerL2: ownerL2,
            tokenIdL2: tokenIdL2,
            decimals: 6,
            inventoryCap: 10_000_000,
            mftStandardVkId: keccak256("mft standard vk"),
            vaultPublicKey: bytes32(uint256(0x654321)),
            universalBridgeVkId: keccak256("universal bridge vk")
        });

        bytes32 recordHash = registry.proposeAsset(record);
        assertEq(uint8(registry.assetStatusByToken(address(token6))), uint8(AssetStatus.Pending));
        vm.expectRevert(abi.encodeWithSelector(EthereumZekoBridge.TokenNotAdded.selector, address(token6)));
        bridge.submitDeposit(address(token6), 1, ZekoAddressLib.pack(0x01020304, false));

        bytes32 settledRoot = keccak256("settled Poseidon registry root");
        bytes32 recordCommitment = bytes32(uint256(991));
        settlement.setAssetRegistryCheckpoint(settledRoot, 1, 1, recordHash, recordCommitment);
        bytes32 wrongRoot = keccak256("wrong root");
        vm.expectRevert(
            abi.encodeWithSelector(
                IZekoAssetRegistry.RegistryCheckpointMismatch.selector,
                wrongRoot,
                uint32(1),
                uint32(1),
                settledRoot,
                uint32(1),
                uint32(1)
            )
        );
        registry.activateAsset(address(token6), wrongRoot, 1);

        registry.activateAsset(address(token6), settledRoot, 1);
        assertTrue(bridge.canonicalTokenRegistered(address(token6)));
        assertEq(bridge.assetIdByToken(address(token6)), record.assetId);
        assertEq(bridge.registryIndexByToken(address(token6)), 0);
        assertEq(bridge.recordCommitmentByToken(address(token6)), recordCommitment);
        assertEq(uint8(registry.assetStatusByToken(address(token6))), uint8(AssetStatus.Active));

        registry.setAssetStatus(address(token6), AssetStatus.Paused);
        (,, bool allowed) = bridge.allowedToken(address(token6));
        assertFalse(allowed);
        assertEq(registry.assetRecord(address(token6)).assetId, record.assetId);
    }

    function test_UniversalAssetRejectsDecimalsAboveZekoMaximum() public {
        bytes32 ownerL2 = bytes32(uint256(0x123456));
        bytes32 tokenIdL2 = keccak256("high-decimal token id");
        AssetRecord memory record = AssetRecord({
            schemaVersion: 1,
            registryIndex: 0,
            assetId: bridge.computeERC20AssetId(address(token18), ownerL2, tokenIdL2, 18),
            ethereumToken: address(token18),
            tokenOwnerL2: ownerL2,
            tokenIdL2: tokenIdL2,
            decimals: 18,
            inventoryCap: 10_000_000,
            mftStandardVkId: keccak256("mft standard vk"),
            vaultPublicKey: bytes32(uint256(0x654321)),
            universalBridgeVkId: keccak256("universal bridge vk")
        });

        vm.expectRevert(IZekoAssetRegistry.InvalidAssetRecord.selector);
        registry.proposeAsset(record);
    }

    function test_UniversalRegistryRejectsDuplicateL2Identity() public {
        bytes32 ownerL2 = bytes32(uint256(0x123456));
        bytes32 tokenIdL2 = keccak256("shared token id");
        AssetRecord memory first = AssetRecord({
            schemaVersion: 1,
            registryIndex: 0,
            assetId: bridge.computeERC20AssetId(address(token6), ownerL2, tokenIdL2, 6),
            ethereumToken: address(token6),
            tokenOwnerL2: ownerL2,
            tokenIdL2: tokenIdL2,
            decimals: 6,
            inventoryCap: 10_000_000,
            mftStandardVkId: keccak256("mft standard vk"),
            vaultPublicKey: bytes32(uint256(0x654321)),
            universalBridgeVkId: keccak256("universal bridge vk")
        });
        registry.proposeAsset(first);

        TestERC20 duplicateToken = new TestERC20("Duplicate token", "DUP", 6);
        AssetRecord memory duplicate = first;
        duplicate.registryIndex = 1;
        duplicate.ethereumToken = address(duplicateToken);
        duplicate.assetId = bridge.computeERC20AssetId(address(duplicateToken), ownerL2, tokenIdL2, 6);
        vm.expectRevert(
            abi.encodeWithSelector(
                IZekoAssetRegistry.AssetIdentityAlreadyProposed.selector, keccak256(abi.encode(ownerL2, tokenIdL2))
            )
        );
        registry.proposeAsset(duplicate);
    }

    function test_UniversalRegistryCanCancelOnlyUnsettledTailProposal() public {
        bytes32 ownerL2 = bytes32(uint256(0x123456));
        bytes32 tokenIdL2 = keccak256("malformed token id");
        AssetRecord memory malformed = AssetRecord({
            schemaVersion: 1,
            registryIndex: 0,
            assetId: bridge.computeERC20AssetId(address(token6), ownerL2, tokenIdL2, 6),
            ethereumToken: address(token6),
            tokenOwnerL2: ownerL2,
            tokenIdL2: tokenIdL2,
            decimals: 6,
            inventoryCap: 10_000_000,
            mftStandardVkId: keccak256("mft standard vk"),
            vaultPublicKey: bytes32(uint256(0x654321)),
            universalBridgeVkId: keccak256("universal bridge vk")
        });

        bytes32 recordHash = registry.proposeAsset(malformed);
        bytes32 laterOwnerL2 = bytes32(uint256(0x234567));
        bytes32 laterTokenIdL2 = keccak256("later malformed token id");
        AssetRecord memory later = AssetRecord({
            schemaVersion: malformed.schemaVersion,
            registryIndex: 1,
            assetId: bridge.computeERC20AssetId(address(token9), laterOwnerL2, laterTokenIdL2, token9.decimals()),
            ethereumToken: address(token9),
            tokenOwnerL2: laterOwnerL2,
            tokenIdL2: laterTokenIdL2,
            decimals: token9.decimals(),
            inventoryCap: malformed.inventoryCap,
            mftStandardVkId: malformed.mftStandardVkId,
            vaultPublicKey: malformed.vaultPublicKey,
            universalBridgeVkId: malformed.universalBridgeVkId
        });
        registry.proposeAsset(later);

        vm.prank(alice);
        vm.expectRevert(abi.encodeWithSelector(IZekoAssetRegistry.UnauthorizedAssetRegistryCaller.selector, alice));
        registry.cancelLastPendingAsset(address(token9));

        vm.expectRevert(
            abi.encodeWithSelector(
                IZekoAssetRegistry.AssetProposalNotTail.selector, address(token6), uint32(0), uint32(2)
            )
        );
        registry.cancelLastPendingAsset(address(token6));
        registry.cancelLastPendingAsset(address(token9));
        registry.cancelLastPendingAsset(address(token6));

        assertEq(registry.proposedAssetCount(), 0);
        assertEq(uint8(registry.assetStatusByToken(address(token6))), uint8(AssetStatus.None));
        assertFalse(registry.proposedAssetRecord(recordHash));
        assertFalse(registry.proposedAssetId(malformed.assetId));
        assertFalse(registry.proposedL2TokenIdentity(keccak256(abi.encode(ownerL2, tokenIdL2))));

        registry.proposeAsset(malformed);
        settlement.setAssetRegistryCheckpoint(
            keccak256("settled malformed registry root"), 1, 1, recordHash, bytes32(uint256(991))
        );
        vm.expectRevert(
            abi.encodeWithSelector(
                IZekoAssetRegistry.AssetProposalAlreadySettled.selector, recordHash, uint32(0), uint32(1)
            )
        );
        registry.cancelLastPendingAsset(address(token6));
    }

    function test_AssetRecordBatchMatchesSharedV2GoldenVector() public view {
        bytes32 recordHash = bytes32(uint256(0x1111111111111111111111111111111111111111111111111111111111111111));
        bytes32 recordCommitment = bytes32(uint256(0x2222222222222222222222222222222222222222222222222222222222222222));
        bytes32 leaf = keccak256(
            abi.encodePacked(registryModule.ASSET_RECORD_BATCH_LEAF_V2_DOMAIN(), recordHash, recordCommitment)
        );
        assertEq(leaf, 0xd3c4982b15c04f3dc43bb070575355bb0155b6298b01f5858a51431c9dabd1fc);

        bytes32 root = leaf;
        bytes32 zero;
        for (uint256 level = 0; level < 8; level++) {
            root = keccak256(abi.encodePacked(registryModule.ASSET_RECORD_BATCH_NODE_V1_DOMAIN(), root, zero));
            zero = keccak256(abi.encodePacked(registryModule.ASSET_RECORD_BATCH_NODE_V1_DOMAIN(), zero, zero));
        }
        assertEq(root, 0x3585343cc90a5d46ee8d6fa33b86040ea6ff3845175e8fb64640469392a10a3b);
    }

    function test_UniversalRegistryActivatesTwoExactRecordsFromOneSettlementBatch() public {
        bytes32 sharedVault = bytes32(uint256(0x654321));
        bytes32 mftVk = keccak256("mft standard vk");
        bytes32 universalVk = keccak256("universal bridge vk");
        TestERC20 secondToken = new TestERC20("Second token", "SECOND", 6);
        AssetRecord memory first = AssetRecord({
            schemaVersion: 1,
            registryIndex: 0,
            assetId: bridge.computeERC20AssetId(
                address(token6), bytes32(uint256(0x111111)), keccak256("token id 0"), 6
            ),
            ethereumToken: address(token6),
            tokenOwnerL2: bytes32(uint256(0x111111)),
            tokenIdL2: keccak256("token id 0"),
            decimals: 6,
            inventoryCap: 10_000_000,
            mftStandardVkId: mftVk,
            vaultPublicKey: sharedVault,
            universalBridgeVkId: universalVk
        });
        AssetRecord memory second = AssetRecord({
            schemaVersion: 1,
            registryIndex: 1,
            assetId: bridge.computeERC20AssetId(
                address(secondToken), bytes32(uint256(0x222222)), keccak256("token id 1"), 6
            ),
            ethereumToken: address(secondToken),
            tokenOwnerL2: bytes32(uint256(0x222222)),
            tokenIdL2: keccak256("token id 1"),
            decimals: 6,
            inventoryCap: type(uint64).max,
            mftStandardVkId: mftVk,
            vaultPublicKey: sharedVault,
            universalBridgeVkId: universalVk
        });

        bytes32 firstHash = registry.proposeAsset(first);
        bytes32 secondHash = registry.proposeAsset(second);
        bytes32 firstCommitment = bytes32(uint256(991));
        bytes32 secondCommitment = bytes32(uint256(992));
        (bytes32 batchRoot, bytes32[8] memory firstProof, bytes32[8] memory secondProof) =
            _twoAssetRecordBatch(firstHash, firstCommitment, secondHash, secondCommitment);
        bytes32 registryRoot = keccak256("settled two-record Poseidon registry root");
        settlement.setAssetRegistryRecordBatch(1, registryRoot, 2, 1, batchRoot, 2);

        vm.expectRevert();
        registry.activateAssetFromBatch(address(secondToken), 1, secondCommitment, firstProof);
        vm.expectRevert();
        registry.activateAssetFromBatch(address(token6), 1, secondCommitment, firstProof);

        registry.activateAssetFromBatch(address(token6), 1, firstCommitment, firstProof);
        registry.activateAssetFromBatch(address(secondToken), 1, secondCommitment, secondProof);

        assertEq(uint8(registry.assetStatusByToken(address(token6))), uint8(AssetStatus.Active));
        assertEq(uint8(registry.assetStatusByToken(address(secondToken))), uint8(AssetStatus.Active));
        assertEq(registry.assetRecord(address(token6)).vaultPublicKey, sharedVault);
        assertEq(registry.assetRecord(address(secondToken)).vaultPublicKey, sharedVault);
        assertEq(registry.assetRecord(address(token6)).universalBridgeVkId, universalVk);
        assertEq(registry.assetRecord(address(secondToken)).universalBridgeVkId, universalVk);
        assertNotEq(bridge.assetIdByToken(address(token6)), bridge.assetIdByToken(address(secondToken)));
        assertNotEq(bridge.zekoTokenIdByToken(address(token6)), bridge.zekoTokenIdByToken(address(secondToken)));
        assertEq(bridge.recordCommitmentByToken(address(token6)), firstCommitment);
        assertEq(bridge.recordCommitmentByToken(address(secondToken)), secondCommitment);
    }

    function test_SubmitDepositRejectsAmountOutsideZekoUInt64() public {
        _activateCanonicalToken(
            token9, bytes32(uint256(0x123456)), keccak256("zeko fungible token id"), type(uint64).max
        );

        uint256 amount = uint256(type(uint64).max) + 1;
        token9.mint(alice, amount);
        vm.startPrank(alice);
        token9.approve(address(bridge), amount);
        vm.expectRevert(abi.encodeWithSelector(EthereumZekoBridge.AmountExceedsZekoUInt64.selector, amount));
        bridge.submitDeposit(address(token9), amount, ZekoAddressLib.pack(0x01020304, false));
        vm.stopPrank();
    }

    function test_SubmitDepositRejectsLiabilityAboveRegisteredCapacity() public {
        uint64 depositCap = 2_000_000;
        _activateCanonicalToken(token6, bytes32(uint256(0x123456)), keccak256("zeko fungible token id"), depositCap);

        vm.startPrank(alice);
        token6.approve(address(bridge), uint256(depositCap) + 1);
        bridge.submitDeposit(address(token6), depositCap, ZekoAddressLib.pack(0x01020304, false));
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.TokenDepositCapExceeded.selector,
                address(token6),
                uint256(depositCap),
                uint256(depositCap) + 1
            )
        );
        bridge.submitDeposit(address(token6), 1, ZekoAddressLib.pack(0x01020304, false));
        vm.stopPrank();
    }

    function test_DepositETHCanonicalUsesInfiniteTimeoutAndTracksLiability() public {
        ZekoAddress recipient = ZekoAddressLib.pack(0x1234, false);
        vm.deal(alice, 1 ether);
        vm.prank(alice);
        (uint64 nonce, bytes32 leaf,) = bridge.depositETH{value: 1 ether}(recipient);

        assertEq(nonce, 1);
        assertEq(leaf, bridge.computeDepositLeaf(address(0), recipient, 1_000_000_000, type(uint32).max, 1));
        assertEq(bridge.nativeEscrowLiability(), 1 ether);
    }

    function test_ClaimNativeWithdrawalUsesSettlementRootDelayAndCursor() public {
        ZekoAddress zekoRecipient = ZekoAddressLib.pack(0x1234, false);
        vm.deal(alice, 1 ether);
        vm.prank(alice);
        bridge.depositETH{value: 1 ether}(zekoRecipient);

        bridge.setWithdrawalDelaySlots(5);
        uint64 sequence = 3;
        uint32 startIndex = 7;
        uint64 amount = 1_000_000_000;
        bytes32 actionFieldsHash = keccak256("bound action fields");
        bytes32 leaf = bridge.computeNativeWithdrawalLeaf(startIndex, bob, amount, actionFieldsHash);
        (bytes32 root, bytes32[16] memory proof) = _singleInnerActionTree(leaf);
        settlement.setInnerActionBatch(sequence, root, startIndex, 1, 100);

        settlement.setVirtualSlot(104);
        vm.expectRevert(
            abi.encodeWithSelector(EthereumZekoBridge.WithdrawalNotYetClaimable.selector, uint64(104), uint64(105))
        );
        bridge.claimNativeWithdrawal(sequence, 0, bob, amount, actionFieldsHash, proof);

        settlement.setVirtualSlot(105);
        uint256 bobBefore = bob.balance;
        bridge.claimNativeWithdrawal(sequence, 0, bob, amount, actionFieldsHash, proof);
        assertEq(bob.balance - bobBefore, 1 ether);
        assertEq(bridge.nextWithdrawalIndex(bob), 8);
        assertEq(bridge.nativeEscrowLiability(), 0);

        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.WithdrawalIndexAlreadyProcessed.selector, bob, uint32(8), uint32(7)
            )
        );
        bridge.claimNativeWithdrawal(sequence, 0, bob, amount, actionFieldsHash, proof);
    }

    function test_ClaimERC20WithdrawalUsesAssetBoundSettlementLeaf() public {
        bytes32 zekoTokenOwner = bytes32(uint256(0x123456));
        bytes32 zekoTokenId = keccak256("zeko fungible token id");
        (, bytes32 recordCommitment) = _activateCanonicalToken(token9, zekoTokenOwner, zekoTokenId, type(uint64).max);

        uint64 amount = 2 * 10 ** 9;
        vm.startPrank(alice);
        token9.approve(address(bridge), amount);
        bridge.submitDeposit(address(token9), amount, ZekoAddressLib.pack(0x1234, false));
        vm.stopPrank();

        // Disabling new deposits must not strand already-backed withdrawals.
        registry.setAssetStatus(address(token9), AssetStatus.Paused);
        bridge.setWithdrawalDelaySlots(5);
        uint64 sequence = 4;
        uint32 startIndex = 11;
        bytes32 actionFieldsHash = keccak256("asset-bound action fields");
        bytes32 leaf = bridge.computeERC20WithdrawalLeaf(
            startIndex,
            address(token9),
            0,
            recordCommitment,
            bridge.assetIdByToken(address(token9)),
            bob,
            amount,
            actionFieldsHash
        );
        (bytes32 root, bytes32[16] memory proof) = _singleInnerActionTree(leaf);
        settlement.setVirtualSlot(105);

        bytes32 wrongIndexLeaf = bridge.computeERC20WithdrawalLeaf(
            startIndex,
            address(token9),
            1,
            recordCommitment,
            bridge.assetIdByToken(address(token9)),
            bob,
            amount,
            actionFieldsHash
        );
        (bytes32 wrongIndexRoot, bytes32[16] memory wrongIndexProof) = _singleInnerActionTree(wrongIndexLeaf);
        settlement.setInnerActionBatch(sequence, wrongIndexRoot, startIndex, 1, 100);
        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.claimERC20Withdrawal(sequence, 0, address(token9), bob, amount, actionFieldsHash, wrongIndexProof);

        bytes32 wrongCommitmentLeaf = bridge.computeERC20WithdrawalLeaf(
            startIndex,
            address(token9),
            0,
            bytes32(uint256(recordCommitment) + 1),
            bridge.assetIdByToken(address(token9)),
            bob,
            amount,
            actionFieldsHash
        );
        (bytes32 wrongCommitmentRoot, bytes32[16] memory wrongCommitmentProof) =
            _singleInnerActionTree(wrongCommitmentLeaf);
        settlement.setInnerActionBatch(sequence, wrongCommitmentRoot, startIndex, 1, 100);
        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.claimERC20Withdrawal(sequence, 0, address(token9), bob, amount, actionFieldsHash, wrongCommitmentProof);

        settlement.setInnerActionBatch(sequence, root, startIndex, 1, 100);
        uint256 bobBefore = token9.balanceOf(bob);
        bridge.claimERC20Withdrawal(sequence, 0, address(token9), bob, amount, actionFieldsHash, proof);

        assertEq(token9.balanceOf(bob) - bobBefore, amount);
        assertEq(bridge.escrowLiabilityByToken(address(token9)), 0);
        assertEq(bridge.nextTokenWithdrawalIndex(address(token9), bob), startIndex + 1);
    }

    function test_ClaimERC20WithdrawalRejectsCrossAssetReplay() public {
        (, bytes32 token9Commitment) = _activateCanonicalToken(
            token9, bytes32(uint256(0x123456)), keccak256("zeko fungible token id 18"), type(uint64).max
        );
        _activateCanonicalToken(
            token6, bytes32(uint256(0x654321)), keccak256("zeko fungible token id 6"), type(uint64).max
        );

        uint64 amount = 2_000_000;
        vm.startPrank(alice);
        token6.approve(address(bridge), amount);
        bridge.submitDeposit(address(token6), amount, ZekoAddressLib.pack(0x1234, false));
        vm.stopPrank();

        uint64 sequence = 5;
        uint32 startIndex = 12;
        bytes32 actionFieldsHash = keccak256("asset-bound action fields");
        bytes32 token9Leaf = bridge.computeERC20WithdrawalLeaf(
            startIndex,
            address(token9),
            0,
            token9Commitment,
            bridge.assetIdByToken(address(token9)),
            bob,
            amount,
            actionFieldsHash
        );
        (bytes32 root, bytes32[16] memory proof) = _singleInnerActionTree(token9Leaf);
        settlement.setInnerActionBatch(sequence, root, startIndex, 1, 100);
        bridge.setWithdrawalDelaySlots(0);
        settlement.setVirtualSlot(100);

        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.claimERC20Withdrawal(sequence, 0, address(token6), bob, amount, actionFieldsHash, proof);
    }

    function test_EmergencyWithdrawCannotDrainCanonicalERC20Liability() public {
        _activateCanonicalToken(
            token9, bytes32(uint256(0x123456)), keccak256("zeko fungible token id"), type(uint64).max
        );

        uint64 amount = 2 * 10 ** 9;
        vm.startPrank(alice);
        token9.approve(address(bridge), amount);
        bridge.submitDeposit(address(token9), amount, ZekoAddressLib.pack(0x1234, false));
        vm.stopPrank();

        vm.expectRevert();
        bridge.emergencyWithdrawToken(address(token9), owner, 1);
    }

    function test_DepositETH_RevertsWhenPrecisionDoesNotFitZekoDecimals() public {
        vm.deal(alice, 1 ether + 1);
        vm.prank(alice);
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidAmountPrecision.selector, address(0), 1 ether + 1, uint8(18), uint8(9)
            )
        );
        bridge.depositETH{value: 1 ether + 1}(ZekoAddressLib.pack(1, false));
    }

    function test_ComputeDepositLeaf_RevertsOnInvalidZekoAddress() public {
        ZekoAddress invalid = ZekoAddress.wrap(ZEKO_FIELD_ORDER);

        vm.expectRevert(ZekoAddressLib.InvalidZekoField.selector);
        bridge.computeDepositLeaf(address(token18), invalid, 1, 1, 1);
    }

    function test_SubmitBridgeTransition_StoresProcessedDepositActionState() public {
        bytes32 oldActionState = keccak256("old deposit action state");
        bytes32 actionState = keccak256("deposit action state");
        settlement.setCurrentActionState(oldActionState);
        ZekoAddress recipient = ZekoAddressLib.pack(123, false);
        bridge.depositETH{value: 1 ether}(recipient);
        bytes memory publicValues = _bridgePublicValues(
            bridge.depositStateByNonce(0),
            bridge.currentDepositState(),
            0,
            bridge.depositNonce(),
            oldActionState,
            actionState,
            1
        );

        bridge.submitBridgeTransition(publicValues, "");

        assertTrue(bridge.processedActionState(actionState));
        assertEq(bridge.bridgedDepositNonce(), 1);
        assertEq(settlement.actionState(), actionState);
    }

    function test_SubmitBridgeTransitionV2_RecordsEveryActionCheckpoint() public {
        bytes32 oldActionState = keccak256("v2 old action state");
        bytes32 intermediateActionState = keccak256("v2 intermediate state");
        bytes32 finalActionState = keccak256("v2 final action state");
        settlement.setCurrentActionState(oldActionState);
        settlement.setOuterActionStateLength(7);

        ZekoAddress recipient = ZekoAddressLib.pack(123, false);
        bridge.depositETH{value: 1 ether}(recipient);
        bridge.depositETH{value: 2 ether}(recipient);

        bytes memory firstAction = abi.encodePacked(
            bytes32(uint256(1)),
            bytes32(uint256(2)),
            bytes32(uint256(3)),
            bytes32(uint256(4)),
            bytes32(uint256(5)),
            intermediateActionState
        );
        bytes memory secondAction = abi.encodePacked(
            bytes32(uint256(6)),
            bytes32(uint256(7)),
            bytes32(uint256(8)),
            bytes32(uint256(9)),
            bytes32(uint256(10)),
            finalActionState
        );
        bytes memory publicValues = abi.encodePacked(
            bytes4(0x5a4b4252),
            uint16(2),
            uint16(0),
            bridge.depositStateByNonce(0),
            bridge.currentDepositState(),
            uint64(0),
            uint64(2),
            oldActionState,
            finalActionState,
            uint32(7),
            uint32(9),
            uint32(2),
            firstAction,
            secondAction
        );

        bridge.submitBridgeTransition(publicValues, "");

        assertEq(settlement.actionState(), finalActionState);
        assertEq(settlement.outerActionStateLength(), 9);
        assertEq(settlement.appendCalls(), 2);
        assertEq(bridge.bridgedDepositNonce(), 2);
        assertTrue(bridge.processedActionState(finalActionState));
    }

    function test_SubmitBridgeTransition_RevertsWhenNotProver() public {
        bytes32 oldActionState = keccak256("old deposit action state");
        bytes32 actionState = keccak256("deposit action state");
        bytes32 proverRole = bridge.PROVER_ROLE();
        bytes memory publicValues = _bridgePublicValues(
            bridge.currentDepositState(),
            bridge.currentDepositState(),
            bridge.depositNonce(),
            bridge.depositNonce(),
            oldActionState,
            actionState,
            0
        );

        vm.expectRevert(
            abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, alice, proverRole)
        );
        vm.prank(alice);
        bridge.submitBridgeTransition(publicValues, "");
    }

    function _bridgePublicValues(
        bytes32 ethereumStateBefore,
        bytes32 ethereumStateAfter,
        uint64 ethereumNonceBefore,
        uint64 ethereumNonceAfter,
        bytes32 zekoActionStateBefore,
        bytes32 zekoActionStateAfter,
        uint32 depositCount
    ) private pure returns (bytes memory publicValues) {
        assert(depositCount <= 1);
        publicValues = abi.encodePacked(
            bytes4(0x5a4b4252),
            uint16(2),
            uint16(0),
            ethereumStateBefore,
            ethereumStateAfter,
            ethereumNonceBefore,
            ethereumNonceAfter,
            zekoActionStateBefore,
            zekoActionStateAfter,
            uint32(0),
            depositCount,
            depositCount
        );
        if (depositCount == 1) {
            publicValues = bytes.concat(
                publicValues,
                abi.encodePacked(
                    bytes32(uint256(1)),
                    bytes32(uint256(2)),
                    bytes32(uint256(3)),
                    bytes32(uint256(4)),
                    bytes32(uint256(5)),
                    zekoActionStateAfter
                )
            );
        }
    }

    function _singleInnerActionTree(bytes32 leaf) private view returns (bytes32 root, bytes32[16] memory proof) {
        root = leaf;
        bytes32 zero;
        for (uint256 level = 0; level < 16; level++) {
            proof[level] = zero;
            root = keccak256(abi.encode(bridge.INNER_ACTION_NODE_V2_DOMAIN(), root, zero));
            zero = keccak256(abi.encode(bridge.INNER_ACTION_NODE_V2_DOMAIN(), zero, zero));
        }
    }

    function _twoAssetRecordBatch(
        bytes32 firstRecordHash,
        bytes32 firstRecordCommitment,
        bytes32 secondRecordHash,
        bytes32 secondRecordCommitment
    ) private view returns (bytes32 root, bytes32[8] memory firstProof, bytes32[8] memory secondProof) {
        bytes32[256] memory nodes;
        nodes[0] = keccak256(
            abi.encodePacked(registryModule.ASSET_RECORD_BATCH_LEAF_V2_DOMAIN(), firstRecordHash, firstRecordCommitment)
        );
        nodes[1] = keccak256(
            abi.encodePacked(
                registryModule.ASSET_RECORD_BATCH_LEAF_V2_DOMAIN(), secondRecordHash, secondRecordCommitment
            )
        );
        uint256 width = 256;
        uint256 firstIndex;
        uint256 secondIndex = 1;
        for (uint256 level = 0; level < 8; level++) {
            firstProof[level] = nodes[firstIndex ^ 1];
            secondProof[level] = nodes[secondIndex ^ 1];
            for (uint256 index = 0; index < width; index += 2) {
                nodes[index / 2] = keccak256(
                    abi.encodePacked(registryModule.ASSET_RECORD_BATCH_NODE_V1_DOMAIN(), nodes[index], nodes[index + 1])
                );
            }
            width >>= 1;
            firstIndex >>= 1;
            secondIndex >>= 1;
        }
        root = nodes[0];
    }
}
