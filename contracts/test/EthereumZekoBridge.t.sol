// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {Vm} from "forge-std/Vm.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {
    IAccessControl
} from "@openzeppelin/contracts/access/IAccessControl.sol";
import {
    ERC1967Proxy
} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";

import {EthereumZekoBridge} from "../src/EthereumZekoBridge.sol";
import {
    AssetRecord,
    AssetStatus,
    IZekoAssetRegistry,
    ZekoAssetRegistry
} from "../src/ZekoAssetRegistry.sol";
import {ZekoAddress, ZekoAddressLib} from "../src/ZekoAddress.sol";
import {ISP1Verifier} from "../src/ZekoSettlement.sol";

contract TestERC20 is ERC20 {
    uint8 private immutable _decimals;

    constructor(
        string memory name_,
        string memory symbol_,
        uint8 decimals_
    ) ERC20(name_, symbol_) {
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

    function appendOuterWitnessBatch(
        bytes32 stateBefore,
        bytes32 stateAfter,
        uint32 count
    ) external {
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
        bytes32 recordHash
    ) external {
        assetRegistryRoot = root;
        assetRegistryCount = count;
        assetRegistrySchemaVersion = schemaVersion;
        settledAssetRecord[recordHash] = true;
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

    function setInnerActionBatch(
        uint64 sequence,
        bytes32 root,
        uint32 startIndex,
        uint32 count,
        uint32 commitSlotUpper
    ) external {
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

    function innerActionBatch(
        uint64 sequence
    )
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

    function setActionStateValid(bytes32 actionState, bool valid) external {
        validActionState[actionState] = valid;
    }

    function setL2ActionStateInfo(
        bytes32 actionState,
        uint64 index,
        bool valid
    ) external {
        l2ActionStateIndex[actionState] = index;
        validActionState[actionState] = valid;
    }

    function isActionStateValid(
        bytes32 actionState
    ) external view returns (bool) {
        return validActionState[actionState];
    }

    function l2ActionStateInfo(
        bytes32 actionState
    ) external view returns (uint64 index, bool valid) {
        return (l2ActionStateIndex[actionState], validActionState[actionState]);
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

    function verifyProof(
        bytes32 programVKey,
        bytes calldata publicValues,
        bytes calldata proofBytes
    ) external view override {
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
    bytes32 internal bridgeProgramVKey = keccak256("bridge program vkey");
    bytes32 internal withdrawProgramVKey = keccak256("withdraw program vkey");

    address internal owner = address(this);
    address internal alice = address(0xA11CE);
    address internal bob = address(0xB0B);

    function setUp() public {
        settlement = new MockSettlementVerifier();
        sp1Verifier = new MockSP1Verifier();
        registryModule = new ZekoAssetRegistry();
        EthereumZekoBridge implementation = new EthereumZekoBridge(
            registryModule
        );
        ERC1967Proxy proxy = new ERC1967Proxy(
            address(implementation),
            abi.encodeCall(
                EthereumZekoBridge.initialize,
                (
                    owner,
                    address(settlement),
                    address(sp1Verifier),
                    bridgeProgramVKey,
                    address(sp1Verifier),
                    withdrawProgramVKey
                )
            )
        );
        bridge = EthereumZekoBridge(payable(address(proxy)));
        registry = IZekoAssetRegistry(address(proxy));
        bridge.setLegacyDepositEnabled(true);
        bridge.setLegacyWithdrawEnabled(true);
        token18 = new TestERC20("Token18", "TK18", 18);
        token6 = new TestERC20("Token6", "TK6", 6);

        token18.mint(alice, 100 ether);
        token6.mint(alice, 100 * 10 ** 6);
    }

    function test_SetUp_ConfiguresNativeETH() public view {
        (uint8 zekoDecimals, uint8 ethereumDecimals, bool allowed) = bridge
            .allowedToken(address(0));

        assertEq(zekoDecimals, 9);
        assertEq(ethereumDecimals, 18);
        assertTrue(allowed);
        assertEq(address(bridge.settlementVerifier()), address(settlement));
        assertEq(address(bridge.bridgeVerifier()), address(sp1Verifier));
        assertEq(bridge.bridgeProgramVKey(), bridgeProgramVKey);
        assertEq(address(bridge.withdrawVerifier()), address(sp1Verifier));
        assertEq(bridge.withdrawProgramVKey(), withdrawProgramVKey);
        assertTrue(bridge.legacyDepositEnabled());
        assertTrue(bridge.hasRole(bridge.DEFAULT_ADMIN_ROLE(), owner));
        assertTrue(bridge.hasRole(bridge.ADMIN_ROLE(), owner));
        assertTrue(bridge.hasRole(bridge.PROVER_ROLE(), owner));
        assertTrue(bridge.hasRole(bridge.UPGRADER_ROLE(), owner));
    }

    function test_Upgrade_RevertsWhenNotUpgrader() public {
        EthereumZekoBridge newImplementation = new EthereumZekoBridge(
            registryModule
        );
        bytes32 upgraderRole = bridge.UPGRADER_ROLE();

        vm.expectRevert(
            abi.encodeWithSelector(
                IAccessControl.AccessControlUnauthorizedAccount.selector,
                alice,
                upgraderRole
            )
        );
        vm.prank(alice);
        bridge.upgradeToAndCall(address(newImplementation), "");
    }

    function test_Upgrade_AllowsUpgrader() public {
        EthereumZekoBridge newImplementation = new EthereumZekoBridge(
            registryModule
        );

        bridge.upgradeToAndCall(address(newImplementation), "");

        assertEq(bridge.currentDepositState(), bridge.INITIAL_DEPOSIT_STATE());
    }

    function test_AddToken_StoresDecimals() public {
        bridge.addToken(address(token18), true, 9, 18);

        (uint8 zekoDecimals, uint8 ethereumDecimals, bool allowed) = bridge
            .allowedToken(address(token18));

        assertEq(zekoDecimals, 9);
        assertEq(ethereumDecimals, 18);
        assertTrue(allowed);
    }

    function test_SetTokenAllowed_CanToggleAllowedAfterInitialization() public {
        bridge.addToken(address(token18), true, 9, 18);
        bridge.setTokenAllowed(address(token18), false);

        (uint8 zekoDecimals, uint8 ethereumDecimals, bool allowed) = bridge
            .allowedToken(address(token18));

        assertEq(zekoDecimals, 9);
        assertEq(ethereumDecimals, 18);
        assertFalse(allowed);
    }

    function test_AddToken_RevertsWhenNotOwner() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                IAccessControl.AccessControlUnauthorizedAccount.selector,
                alice,
                bridge.ADMIN_ROLE()
            )
        );
        vm.prank(alice);
        bridge.addToken(address(token18), true, 9, 18);
    }

    function test_AddToken_RevertsWhenZekoDecimalsTooHigh() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidZekoDecimals.selector,
                uint8(10)
            )
        );
        bridge.addToken(address(token18), true, 10, 18);
    }

    function test_AddToken_RevertsWhenEthereumDecimalsMismatch() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidEthereumDecimals.selector,
                address(token18),
                uint8(6),
                uint8(18)
            )
        );
        bridge.addToken(address(token18), true, 9, 6);
    }

    function test_AddToken_RevertsWhenTokenAlreadyAdded() public {
        bridge.addToken(address(token18), true, 9, 18);

        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.TokenAlreadyAdded.selector,
                address(token18)
            )
        );
        bridge.addToken(address(token18), true, 8, 18);
    }

    function test_AddToken_RevertsWhenNativeTokenAlreadyAdded() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.TokenAlreadyAdded.selector,
                address(0)
            )
        );
        bridge.addToken(address(0), true, 8, 18);
    }

    function test_SetTokenAllowed_RevertsWhenTokenNotAdded() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.TokenNotAdded.selector,
                address(token18)
            )
        );
        bridge.setTokenAllowed(address(token18), true);
    }

    function test_SetTokenAllowed_RevertsWhenNotOwner() public {
        bridge.addToken(address(token18), true, 9, 18);

        vm.expectRevert(
            abi.encodeWithSelector(
                IAccessControl.AccessControlUnauthorizedAccount.selector,
                alice,
                bridge.ADMIN_ROLE()
            )
        );
        vm.prank(alice);
        bridge.setTokenAllowed(address(token18), false);
    }

    function test_Deposit_SerializesBridgeAddressAndNormalizedAmount() public {
        bridge.addToken(address(token18), true, 9, 18);

        uint256 amount = 2 ether;
        uint64 timeout = 123456;
        ZekoAddress recipient = ZekoAddressLib.pack(0x01020304, false);

        vm.startPrank(alice);
        token18.approve(address(bridge), amount);
        (uint64 nonce, bytes32 leaf, bytes32 newState) = bridge.deposit(
            address(token18),
            amount,
            recipient,
            timeout
        );
        vm.stopPrank();

        bytes32 expectedLeaf = keccak256(
            abi.encode(
                bridge.DEPOSIT_LEAF_DOMAIN(),
                block.chainid,
                address(bridge),
                address(token18),
                recipient,
                2 * 10 ** 9,
                timeout,
                uint64(1)
            )
        );

        assertEq(nonce, 1);
        assertEq(leaf, expectedLeaf);
        assertEq(newState, bridge.currentDepositState());
        assertEq(bridge.totalDepositedByToken(address(token18)), amount);
    }

    function test_SubmitDepositRecordsCanonicalERC20WitnessInput() public {
        bytes32 zekoTokenOwner = bytes32(uint256(0x123456));
        bytes32 zekoTokenId = keccak256("zeko fungible token id");
        bridge.registerToken(
            address(token18),
            zekoTokenOwner,
            zekoTokenId,
            18,
            18,
            type(uint64).max
        );
        bridge.setLegacyDepositEnabled(false);

        uint256 amount = 2 ether;
        ZekoAddress recipient = ZekoAddressLib.pack(0x01020304, false);

        vm.startPrank(alice);
        token18.approve(address(bridge), amount);
        vm.recordLogs();
        (uint64 nonce, bytes32 leaf, bytes32 newState) = bridge.submitDeposit(
            address(token18),
            amount,
            recipient
        );
        Vm.Log[] memory logs = vm.getRecordedLogs();
        vm.stopPrank();

        bytes32 bridgeDepositSignature = keccak256(
            "BridgeDeposit(uint64,bytes32,bytes32,bytes32,address,address,uint256,uint256,uint256,uint64)"
        );
        bytes32 erc20DepositSignature = keccak256(
            "ERC20DepositSubmitted(uint64,bytes32,bytes32,bytes32,address,address,uint256,uint64,uint64)"
        );
        bool foundWitnessEvent;
        bool foundCanonicalEvent;
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
                ) = abi.decode(
                        logs[i].data,
                        (
                            bytes32,
                            address,
                            address,
                            uint256,
                            uint256,
                            uint256,
                            uint64
                        )
                    );
                assertEq(eventToken, address(token18));
                assertEq(eventSender, alice);
                assertEq(eventRecipient, ZekoAddress.unwrap(recipient));
                assertEq(eventAmount, amount);
                assertEq(eventZekoAmount, amount);
                assertEq(eventTimeout, type(uint32).max);
            } else if (logs[i].topics[0] == erc20DepositSignature) {
                foundCanonicalEvent = true;
                assertEq(uint64(uint256(logs[i].topics[1])), nonce);
                assertEq(
                    logs[i].topics[2],
                    bridge.assetIdByToken(address(token18))
                );
                assertEq(logs[i].topics[3], leaf);
                (
                    bytes32 eventState,
                    address eventToken,
                    address eventSender,
                    uint256 eventRecipient,
                    uint64 eventAmount,
                    uint64 eventTimeout
                ) = abi.decode(
                        logs[i].data,
                        (bytes32, address, address, uint256, uint64, uint64)
                    );
                assertEq(eventState, newState);
                assertEq(eventToken, address(token18));
                assertEq(eventSender, alice);
                assertEq(eventRecipient, ZekoAddress.unwrap(recipient));
                assertEq(eventAmount, amount);
                assertEq(eventTimeout, type(uint32).max);
            }
        }
        assertTrue(foundWitnessEvent);
        assertTrue(foundCanonicalEvent);

        assertEq(nonce, 1);
        assertEq(
            leaf,
            bridge.computeERC20DepositLeaf(
                address(token18),
                bridge.assetIdByToken(address(token18)),
                recipient,
                uint64(amount),
                type(uint32).max,
                nonce
            )
        );
        assertEq(newState, bridge.currentDepositState());
        assertEq(token18.balanceOf(address(bridge)), amount);
        assertEq(bridge.escrowLiabilityByToken(address(token18)), amount);
        assertEq(
            bridge.zekoTokenOwnerByToken(address(token18)),
            zekoTokenOwner
        );
        assertEq(bridge.zekoTokenIdByToken(address(token18)), zekoTokenId);
        assertEq(bridge.depositCapByToken(address(token18)), type(uint64).max);
        assertEq(
            bridge.assetIdByToken(address(token18)),
            bridge.computeERC20AssetId(
                address(token18),
                zekoTokenOwner,
                zekoTokenId,
                18
            )
        );
    }

    function test_UniversalAssetRemainsPendingUntilMatchingSettlement() public {
        bytes32 ownerL2 = bytes32(uint256(0x123456));
        bytes32 tokenIdL2 = keccak256("universal token id");
        AssetRecord memory record = AssetRecord({
                schemaVersion: 1,
                registryIndex: 0,
                assetId: bridge.computeERC20AssetId(
                    address(token6),
                    ownerL2,
                    tokenIdL2,
                    6
                ),
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
        assertEq(
            uint8(registry.assetStatusByToken(address(token6))),
            uint8(AssetStatus.Pending)
        );
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.TokenNotAdded.selector,
                address(token6)
            )
        );
        bridge.submitDeposit(
            address(token6),
            1,
            ZekoAddressLib.pack(0x01020304, false)
        );

        bytes32 settledRoot = keccak256("settled Poseidon registry root");
        settlement.setAssetRegistryCheckpoint(settledRoot, 1, 1, recordHash);
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
        assertEq(
            uint8(registry.assetStatusByToken(address(token6))),
            uint8(AssetStatus.Active)
        );

        registry.setAssetStatus(address(token6), AssetStatus.Paused);
        (, , bool allowed) = bridge.allowedToken(address(token6));
        assertFalse(allowed);
        assertEq(registry.assetRecord(address(token6)).assetId, record.assetId);
    }

    function test_UniversalAssetRejectsDecimalsAboveZekoMaximum() public {
        bytes32 ownerL2 = bytes32(uint256(0x123456));
        bytes32 tokenIdL2 = keccak256("high-decimal token id");
        AssetRecord memory record = AssetRecord({
                schemaVersion: 1,
                registryIndex: 0,
                assetId: bridge.computeERC20AssetId(
                    address(token18),
                    ownerL2,
                    tokenIdL2,
                    18
                ),
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
                assetId: bridge.computeERC20AssetId(
                    address(token6),
                    ownerL2,
                    tokenIdL2,
                    6
                ),
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
        duplicate.assetId = bridge.computeERC20AssetId(
            address(duplicateToken),
            ownerL2,
            tokenIdL2,
            6
        );
        vm.expectRevert(
            abi.encodeWithSelector(
                IZekoAssetRegistry.AssetIdentityAlreadyProposed.selector,
                keccak256(abi.encode(ownerL2, tokenIdL2))
            )
        );
        registry.proposeAsset(duplicate);
    }

    function test_UniversalRegistryActivatesTwoExactRecordsFromOneSettlementBatch()
        public
    {
        bytes32 sharedVault = bytes32(uint256(0x654321));
        bytes32 mftVk = keccak256("mft standard vk");
        bytes32 universalVk = keccak256("universal bridge vk");
        TestERC20 secondToken = new TestERC20("Second token", "SECOND", 6);
        AssetRecord memory first = AssetRecord({
                schemaVersion: 1,
                registryIndex: 0,
                assetId: bridge.computeERC20AssetId(
                    address(token6),
                    bytes32(uint256(0x111111)),
                    keccak256("token id 0"),
                    6
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
                    address(secondToken),
                    bytes32(uint256(0x222222)),
                    keccak256("token id 1"),
                    6
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
        (
            bytes32 batchRoot,
            bytes32[8] memory firstProof,
            bytes32[8] memory secondProof
        ) = _twoAssetRecordBatch(firstHash, secondHash);
        bytes32 registryRoot = keccak256(
            "settled two-record Poseidon registry root"
        );
        settlement.setAssetRegistryRecordBatch(
            1,
            registryRoot,
            2,
            1,
            batchRoot,
            2
        );

        vm.expectRevert();
        registry.activateAssetFromBatch(address(secondToken), 1, firstProof);

        registry.activateAssetFromBatch(address(token6), 1, firstProof);
        registry.activateAssetFromBatch(address(secondToken), 1, secondProof);

        assertEq(
            uint8(registry.assetStatusByToken(address(token6))),
            uint8(AssetStatus.Active)
        );
        assertEq(
            uint8(registry.assetStatusByToken(address(secondToken))),
            uint8(AssetStatus.Active)
        );
        assertEq(
            registry.assetRecord(address(token6)).vaultPublicKey,
            sharedVault
        );
        assertEq(
            registry.assetRecord(address(secondToken)).vaultPublicKey,
            sharedVault
        );
        assertEq(
            registry.assetRecord(address(token6)).universalBridgeVkId,
            universalVk
        );
        assertEq(
            registry.assetRecord(address(secondToken)).universalBridgeVkId,
            universalVk
        );
        assertNotEq(
            bridge.assetIdByToken(address(token6)),
            bridge.assetIdByToken(address(secondToken))
        );
        assertNotEq(
            bridge.zekoTokenIdByToken(address(token6)),
            bridge.zekoTokenIdByToken(address(secondToken))
        );
    }

    function test_SubmitDepositRejectsAmountOutsideZekoUInt64() public {
        bridge.registerToken(
            address(token18),
            bytes32(uint256(0x123456)),
            keccak256("zeko fungible token id"),
            18,
            18,
            type(uint64).max
        );

        uint256 amount = uint256(type(uint64).max) + 1;
        token18.mint(alice, amount);
        vm.startPrank(alice);
        token18.approve(address(bridge), amount);
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.AmountExceedsZekoUInt64.selector,
                amount
            )
        );
        bridge.submitDeposit(
            address(token18),
            amount,
            ZekoAddressLib.pack(0x01020304, false)
        );
        vm.stopPrank();
    }

    function test_SubmitDepositRejectsLiabilityAboveRegisteredCapacity()
        public
    {
        uint64 depositCap = 2_000_000;
        bridge.registerToken(
            address(token18),
            bytes32(uint256(0x123456)),
            keccak256("zeko fungible token id"),
            18,
            18,
            depositCap
        );

        vm.startPrank(alice);
        token18.approve(address(bridge), uint256(depositCap) + 1);
        bridge.submitDeposit(
            address(token18),
            depositCap,
            ZekoAddressLib.pack(0x01020304, false)
        );
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.TokenDepositCapExceeded.selector,
                address(token18),
                uint256(depositCap),
                uint256(depositCap) + 1
            )
        );
        bridge.submitDeposit(
            address(token18),
            1,
            ZekoAddressLib.pack(0x01020304, false)
        );
        vm.stopPrank();
    }

    function test_CanonicalTokenCannotEnterThroughLegacyDeposit() public {
        bridge.registerToken(
            address(token18),
            bytes32(uint256(0x123456)),
            keccak256("zeko fungible token id"),
            18,
            18,
            type(uint64).max
        );

        vm.startPrank(alice);
        token18.approve(address(bridge), 2 ether);
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.CanonicalTokenRequiresSubmitDeposit.selector,
                address(token18)
            )
        );
        bridge.deposit(
            address(token18),
            2 ether,
            ZekoAddressLib.pack(0x01020304, false),
            100
        );
        vm.stopPrank();
    }

    function test_DepositETH_UsesNativeTokenConfig() public {
        uint256 amount = 3 ether;
        uint64 timeout = 777;
        ZekoAddress recipient = ZekoAddressLib.pack(0xdeadbeef, true);

        vm.deal(alice, amount);
        vm.prank(alice);
        (uint64 nonce, bytes32 leaf, bytes32 newState) = bridge.depositETH{
            value: amount
        }(recipient, timeout);

        bytes32 expectedLeaf = keccak256(
            abi.encode(
                bridge.DEPOSIT_LEAF_DOMAIN(),
                block.chainid,
                address(bridge),
                address(0),
                recipient,
                3 * 10 ** 9,
                timeout,
                uint64(1)
            )
        );

        assertEq(nonce, 1);
        assertEq(leaf, expectedLeaf);
        assertEq(newState, bridge.currentDepositState());
        assertEq(bridge.totalDepositedByToken(address(0)), amount);
        assertEq(address(bridge).balance, amount);
    }

    function test_DepositETHCanonicalUsesInfiniteTimeoutAndTracksLiability()
        public
    {
        ZekoAddress recipient = ZekoAddressLib.pack(0x1234, false);
        vm.deal(alice, 1 ether);
        vm.prank(alice);
        (uint64 nonce, bytes32 leaf, ) = bridge.depositETH{value: 1 ether}(
            recipient
        );

        assertEq(nonce, 1);
        assertEq(
            leaf,
            bridge.computeDepositLeaf(
                address(0),
                recipient,
                1_000_000_000,
                type(uint32).max,
                1
            )
        );
        assertEq(bridge.nativeEscrowLiability(), 1 ether);
    }

    function test_LegacyDepositSwitchCannotBlockCanonicalNonceStream() public {
        bridge.setLegacyDepositEnabled(false);
        ZekoAddress recipient = ZekoAddressLib.pack(0x1234, false);

        vm.expectRevert(EthereumZekoBridge.LegacyDepositPathDisabled.selector);
        bridge.depositETH{value: 1 ether}(recipient, 10);

        bridge.depositETH{value: 1 ether}(recipient);
        assertEq(bridge.depositNonce(), 1);
    }

    function test_ClaimNativeWithdrawalUsesSettlementRootDelayAndCursor()
        public
    {
        ZekoAddress zekoRecipient = ZekoAddressLib.pack(0x1234, false);
        vm.deal(alice, 1 ether);
        vm.prank(alice);
        bridge.depositETH{value: 1 ether}(zekoRecipient);

        bridge.setWithdrawalDelaySlots(5);
        uint64 sequence = 3;
        uint32 startIndex = 7;
        uint64 amount = 1_000_000_000;
        bytes32 actionFieldsHash = keccak256("bound action fields");
        bytes32 leaf = bridge.computeNativeWithdrawalLeaf(
            startIndex,
            bob,
            amount,
            actionFieldsHash
        );
        (bytes32 root, bytes32[16] memory proof) = _singleInnerActionTree(leaf);
        settlement.setInnerActionBatch(sequence, root, startIndex, 1, 100);

        settlement.setVirtualSlot(104);
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.WithdrawalNotYetClaimable.selector,
                uint64(104),
                uint64(105)
            )
        );
        bridge.claimNativeWithdrawal(
            sequence,
            0,
            bob,
            amount,
            actionFieldsHash,
            proof
        );

        settlement.setVirtualSlot(105);
        uint256 bobBefore = bob.balance;
        bridge.claimNativeWithdrawal(
            sequence,
            0,
            bob,
            amount,
            actionFieldsHash,
            proof
        );
        assertEq(bob.balance - bobBefore, 1 ether);
        assertEq(bridge.nextWithdrawalIndex(bob), 8);
        assertEq(bridge.nativeEscrowLiability(), 0);

        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.WithdrawalIndexAlreadyProcessed.selector,
                bob,
                uint32(8),
                uint32(7)
            )
        );
        bridge.claimNativeWithdrawal(
            sequence,
            0,
            bob,
            amount,
            actionFieldsHash,
            proof
        );
    }

    function test_ClaimERC20WithdrawalUsesAssetBoundSettlementLeaf() public {
        bytes32 zekoTokenOwner = bytes32(uint256(0x123456));
        bytes32 zekoTokenId = keccak256("zeko fungible token id");
        bridge.registerToken(
            address(token18),
            zekoTokenOwner,
            zekoTokenId,
            18,
            18,
            type(uint64).max
        );

        uint64 amount = 2 ether;
        vm.startPrank(alice);
        token18.approve(address(bridge), amount);
        bridge.submitDeposit(
            address(token18),
            amount,
            ZekoAddressLib.pack(0x1234, false)
        );
        vm.stopPrank();

        // Disabling new deposits must not strand already-backed withdrawals.
        bridge.setTokenAllowed(address(token18), false);
        bridge.setWithdrawalDelaySlots(5);
        uint64 sequence = 4;
        uint32 startIndex = 11;
        bytes32 actionFieldsHash = keccak256("asset-bound action fields");
        bytes32 leaf = bridge.computeERC20WithdrawalLeaf(
            startIndex,
            address(token18),
            bridge.assetIdByToken(address(token18)),
            bob,
            amount,
            actionFieldsHash
        );
        (bytes32 root, bytes32[16] memory proof) = _singleInnerActionTree(leaf);
        settlement.setInnerActionBatch(sequence, root, startIndex, 1, 100);
        settlement.setVirtualSlot(105);

        uint256 bobBefore = token18.balanceOf(bob);
        bridge.claimERC20Withdrawal(
            sequence,
            0,
            address(token18),
            bob,
            amount,
            actionFieldsHash,
            proof
        );

        assertEq(token18.balanceOf(bob) - bobBefore, amount);
        assertEq(bridge.escrowLiabilityByToken(address(token18)), 0);
        assertEq(
            bridge.nextTokenWithdrawalIndex(address(token18), bob),
            startIndex + 1
        );
    }

    function test_ClaimERC20WithdrawalRejectsCrossAssetReplay() public {
        bridge.registerToken(
            address(token18),
            bytes32(uint256(0x123456)),
            keccak256("zeko fungible token id 18"),
            18,
            18,
            type(uint64).max
        );
        bridge.registerToken(
            address(token6),
            bytes32(uint256(0x654321)),
            keccak256("zeko fungible token id 6"),
            6,
            6,
            type(uint64).max
        );

        uint64 amount = 2_000_000;
        vm.startPrank(alice);
        token6.approve(address(bridge), amount);
        bridge.submitDeposit(
            address(token6),
            amount,
            ZekoAddressLib.pack(0x1234, false)
        );
        vm.stopPrank();

        uint64 sequence = 5;
        uint32 startIndex = 12;
        bytes32 actionFieldsHash = keccak256("asset-bound action fields");
        bytes32 token18Leaf = bridge.computeERC20WithdrawalLeaf(
            startIndex,
            address(token18),
            bridge.assetIdByToken(address(token18)),
            bob,
            amount,
            actionFieldsHash
        );
        (bytes32 root, bytes32[16] memory proof) = _singleInnerActionTree(
            token18Leaf
        );
        settlement.setInnerActionBatch(sequence, root, startIndex, 1, 100);
        bridge.setWithdrawalDelaySlots(0);
        settlement.setVirtualSlot(100);

        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.claimERC20Withdrawal(
            sequence,
            0,
            address(token6),
            bob,
            amount,
            actionFieldsHash,
            proof
        );
    }

    function test_EmergencyWithdrawCannotDrainCanonicalERC20Liability() public {
        bridge.registerToken(
            address(token18),
            bytes32(uint256(0x123456)),
            keccak256("zeko fungible token id"),
            18,
            18,
            type(uint64).max
        );

        uint64 amount = 2 ether;
        vm.startPrank(alice);
        token18.approve(address(bridge), amount);
        bridge.submitDeposit(
            address(token18),
            amount,
            ZekoAddressLib.pack(0x1234, false)
        );
        vm.stopPrank();

        vm.expectRevert();
        bridge.emergencyWithdrawToken(address(token18), owner, 1);
    }

    function test_DepositETH_RevertsWhenPrecisionDoesNotFitZekoDecimals()
        public
    {
        vm.deal(alice, 1 ether + 1);
        vm.prank(alice);
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidAmountPrecision.selector,
                address(0),
                1 ether + 1,
                uint8(18),
                uint8(9)
            )
        );
        bridge.depositETH{value: 1 ether + 1}(ZekoAddressLib.pack(1, false), 1);
    }

    function test_Deposit_RevertsWhenPrecisionDoesNotFitZekoDecimals() public {
        bridge.addToken(address(token18), true, 9, 18);

        vm.startPrank(alice);
        token18.approve(address(bridge), 1 ether + 1);
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidAmountPrecision.selector,
                address(token18),
                1 ether + 1,
                uint8(18),
                uint8(9)
            )
        );
        bridge.deposit(
            address(token18),
            1 ether + 1,
            ZekoAddressLib.pack(1, false),
            99
        );
        vm.stopPrank();
    }

    function test_Deposit_ScalesUpWhenEthereumHasFewerDecimals() public {
        bridge.addToken(address(token6), true, 9, 6);

        uint256 amount = 25 * 10 ** 6;
        uint64 timeout = 88;
        ZekoAddress recipient = ZekoAddressLib.pack(0x1234, false);

        vm.startPrank(alice);
        token6.approve(address(bridge), amount);
        (, bytes32 leaf, ) = bridge.deposit(
            address(token6),
            amount,
            recipient,
            timeout
        );
        vm.stopPrank();

        bytes32 expectedLeaf = keccak256(
            abi.encode(
                bridge.DEPOSIT_LEAF_DOMAIN(),
                block.chainid,
                address(bridge),
                address(token6),
                recipient,
                25 * 10 ** 9,
                timeout,
                uint64(1)
            )
        );

        assertEq(leaf, expectedLeaf);
    }

    function test_ComputeDepositLeaf_RevertsOnInvalidZekoAddress() public {
        ZekoAddress invalid = ZekoAddress.wrap(ZEKO_FIELD_ORDER);

        vm.expectRevert(ZekoAddressLib.InvalidZekoField.selector);
        bridge.computeDepositLeaf(address(token18), invalid, 1, 1, 1);
    }

    function test_SubmitWithdrawTransition_RequiresSettlementActionState()
        public
    {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        bytes32 withdrawalRoot = keccak256("withdrawal root");
        bytes32 newWithdrawState = bridge.computeNextWithdrawState(
            bridge.currentWithdrawState(),
            withdrawalRoot,
            1
        );
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        bytes memory publicValues = _withdrawPublicValues(
            oldActionState,
            actionState,
            bridge.currentWithdrawState(),
            newWithdrawState,
            withdrawalRoot,
            1
        );

        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidSettlementActionState.selector,
                actionState
            )
        );
        bridge.submitWithdrawTransition(publicValues, "");
    }

    function test_SubmitBridgeTransition_StoresProcessedDepositActionState()
        public
    {
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
        assertEq(bridge.currentWithdrawState(), bytes32(0));
    }

    function test_SubmitBridgeTransitionV2_RecordsEveryActionCheckpoint()
        public
    {
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
            abi.encodeWithSelector(
                IAccessControl.AccessControlUnauthorizedAccount.selector,
                alice,
                proverRole
            )
        );
        vm.prank(alice);
        bridge.submitBridgeTransition(publicValues, "");
    }

    function test_SubmitWithdrawTransition_StoresWithdrawalRootInfo() public {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        bytes32 withdrawalRoot = keccak256("withdrawal root");
        bytes32 newWithdrawState = bridge.computeNextWithdrawState(
            bridge.currentWithdrawState(),
            withdrawalRoot,
            1
        );
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);
        bytes memory publicValues = _withdrawPublicValues(
            oldActionState,
            actionState,
            bridge.currentWithdrawState(),
            newWithdrawState,
            withdrawalRoot,
            1
        );

        bridge.submitWithdrawTransition(publicValues, "");

        assertTrue(bridge.processedActionState(actionState));
        (
            bytes32 storedWithdrawalRoot,
            bytes32 storedStateBefore,
            bytes32 storedStateAfter,
            uint64 storedOldActionStateIndex,
            uint32 storedWithdrawCount,
            bool valid
        ) = bridge.withdrawalRootInfo(oldActionState);
        assertEq(storedWithdrawalRoot, withdrawalRoot);
        assertEq(storedStateBefore, bytes32(0));
        assertEq(storedStateAfter, newWithdrawState);
        assertEq(storedOldActionStateIndex, 0);
        assertEq(storedWithdrawCount, 1);
        assertTrue(valid);
        assertEq(bridge.currentWithdrawState(), newWithdrawState);
    }

    function test_SubmitWithdrawTransition_RevertsWhenL2ActionStateSkipsIndex()
        public
    {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        bytes32 withdrawalRoot = keccak256("withdrawal root");
        bytes32 newWithdrawState = bridge.computeNextWithdrawState(
            bridge.currentWithdrawState(),
            withdrawalRoot,
            1
        );
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 2, true);
        bytes memory publicValues = _withdrawPublicValues(
            oldActionState,
            actionState,
            bridge.currentWithdrawState(),
            newWithdrawState,
            withdrawalRoot,
            1
        );

        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidL2ActionStateTransition.selector,
                oldActionState,
                actionState
            )
        );
        bridge.submitWithdrawTransition(publicValues, "");
    }

    function test_SubmitWithdrawTransition_RevertsWhenActionStateAlreadyProcessed()
        public
    {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        bytes32 withdrawalRoot = keccak256("withdrawal root");
        bytes32 newWithdrawState = bridge.computeNextWithdrawState(
            bridge.currentWithdrawState(),
            withdrawalRoot,
            1
        );
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);
        bytes memory firstPublicValues = _withdrawPublicValues(
            oldActionState,
            actionState,
            bridge.currentWithdrawState(),
            newWithdrawState,
            withdrawalRoot,
            1
        );

        bridge.submitWithdrawTransition(firstPublicValues, "");

        bytes memory secondPublicValues = _withdrawPublicValues(
            oldActionState,
            actionState,
            newWithdrawState,
            bridge.computeNextWithdrawState(
                newWithdrawState,
                keccak256("next withdrawal root"),
                1
            ),
            keccak256("next withdrawal root"),
            1
        );

        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.ActionStateAlreadyProcessed.selector,
                actionState
            )
        );
        bridge.submitWithdrawTransition(secondPublicValues, "");
    }

    function test_ClaimWithdraw_VerifiesMerkleProofAndTransfersERC20() public {
        bridge.addToken(address(token18), true, 9, 18);
        token18.mint(address(bridge), 10 ether);

        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);

        EthereumZekoBridge.WithdrawClaim memory target = EthereumZekoBridge
            .WithdrawClaim({
                token: _addressField(address(token18)),
                recipient: _addressField(alice),
                amount: bytes32(uint256(2 * 10 ** 9))
            });

        bytes32 leaf0 = bridge.computeWithdrawLeaf(
            _addressField(address(token18)),
            _addressField(bob),
            bytes32(uint256(1 * 10 ** 9))
        );
        bytes32 leaf1 = bridge.computeWithdrawLeaf(
            target.token,
            target.recipient,
            target.amount
        );
        bytes32 leaf2 = bridge.computeWithdrawLeaf(
            _addressField(address(token18)),
            _addressField(address(0xCAFE)),
            bytes32(uint256(3 * 10 ** 9))
        );

        bytes32[] memory leaves = new bytes32[](3);
        leaves[0] = leaf0;
        leaves[1] = leaf1;
        leaves[2] = leaf2;
        bytes32 withdrawalRoot = _merkleRoot(leaves);
        bytes32 state = bridge.computeNextWithdrawState(
            bytes32(0),
            withdrawalRoot,
            3
        );

        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                oldActionState,
                actionState,
                bytes32(0),
                state,
                withdrawalRoot,
                3
            ),
            ""
        );

        uint256 aliceBalanceBefore = token18.balanceOf(alice);

        bridge.claimWithdraw(
            oldActionState,
            target,
            1,
            _merkleProof(leaves, 1)
        );

        assertEq(token18.balanceOf(alice), aliceBalanceBefore + 2 ether);
        assertEq(token18.balanceOf(address(bridge)), 8 ether);

        bytes32 nullifier = bridge.computeWithdrawNullifier(0, 1, leaf1);
        assertTrue(bridge.spentWithdraw(nullifier));
    }

    function test_ClaimWithdraw_RevertsOnDoubleClaim() public {
        bridge.addToken(address(token18), true, 9, 18);
        token18.mint(address(bridge), 10 ether);

        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);

        EthereumZekoBridge.WithdrawClaim memory target = EthereumZekoBridge
            .WithdrawClaim({
                token: _addressField(address(token18)),
                recipient: _addressField(alice),
                amount: bytes32(uint256(2 * 10 ** 9))
            });

        bytes32 leaf = bridge.computeWithdrawLeaf(
            target.token,
            target.recipient,
            target.amount
        );
        bytes32[] memory leaves = new bytes32[](1);
        leaves[0] = leaf;
        bytes32 withdrawalRoot = _merkleRoot(leaves);
        bytes32 state = bridge.computeNextWithdrawState(
            bytes32(0),
            withdrawalRoot,
            1
        );
        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                oldActionState,
                actionState,
                bytes32(0),
                state,
                withdrawalRoot,
                1
            ),
            ""
        );

        bytes32[16] memory proof = _merkleProof(leaves, 0);
        bridge.claimWithdraw(oldActionState, target, 0, proof);

        bytes32 nullifier = bridge.computeWithdrawNullifier(0, 0, leaf);
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.WithdrawAlreadyClaimed.selector,
                nullifier
            )
        );
        bridge.claimWithdraw(oldActionState, target, 0, proof);
    }

    function test_ClaimWithdraw_RevertsOnInvalidMerkleProof() public {
        bridge.addToken(address(token18), true, 9, 18);
        token18.mint(address(bridge), 10 ether);

        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);

        EthereumZekoBridge.WithdrawClaim memory target = EthereumZekoBridge
            .WithdrawClaim({
                token: _addressField(address(token18)),
                recipient: _addressField(alice),
                amount: bytes32(uint256(2 * 10 ** 9))
            });

        bytes32 leaf0 = bridge.computeWithdrawLeaf(
            _addressField(address(token18)),
            _addressField(bob),
            bytes32(uint256(1 * 10 ** 9))
        );
        bytes32 leaf1 = bridge.computeWithdrawLeaf(
            target.token,
            target.recipient,
            target.amount
        );

        bytes32[] memory leaves = new bytes32[](2);
        leaves[0] = leaf0;
        leaves[1] = leaf1;
        bytes32 withdrawalRoot = _merkleRoot(leaves);
        bytes32 state = bridge.computeNextWithdrawState(
            bytes32(0),
            withdrawalRoot,
            2
        );
        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                oldActionState,
                actionState,
                bytes32(0),
                state,
                withdrawalRoot,
                2
            ),
            ""
        );

        bytes32[16] memory proof = _merkleProof(leaves, 1);
        proof[0] = keccak256("invalid sibling");

        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.claimWithdraw(oldActionState, target, 1, proof);
    }

    function test_ClaimWithdraw_RevertsWhenIndexExceedsWithdrawCount() public {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);

        EthereumZekoBridge.WithdrawClaim memory target = EthereumZekoBridge
            .WithdrawClaim({
                token: bytes32(0),
                recipient: _addressField(alice),
                amount: bytes32(uint256(1))
            });
        bytes32 leaf = bridge.computeWithdrawLeaf(
            target.token,
            target.recipient,
            target.amount
        );
        bytes32[] memory leaves = new bytes32[](1);
        leaves[0] = leaf;
        bytes32 withdrawalRoot = _merkleRoot(leaves);

        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                oldActionState,
                actionState,
                bytes32(0),
                bridge.computeNextWithdrawState(bytes32(0), withdrawalRoot, 1),
                withdrawalRoot,
                1
            ),
            ""
        );

        (, , , , uint32 withdrawCount, bool valid) = bridge.withdrawalRootInfo(
            oldActionState
        );
        assertEq(withdrawCount, 1);
        assertTrue(valid);
        bytes32[16] memory proof = _merkleProof(leaves, 0);

        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.claimWithdraw(oldActionState, target, 1, proof);
    }

    function test_ClaimWithdraw_RevertsForUnknownOldActionState() public {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);

        EthereumZekoBridge.WithdrawClaim memory target = EthereumZekoBridge
            .WithdrawClaim({
                token: bytes32(0),
                recipient: _addressField(alice),
                amount: bytes32(uint256(1))
            });
        bytes32 leaf = bridge.computeWithdrawLeaf(
            target.token,
            target.recipient,
            target.amount
        );
        bytes32[] memory leaves = new bytes32[](1);
        leaves[0] = leaf;
        bytes32 withdrawalRoot = _merkleRoot(leaves);

        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                oldActionState,
                actionState,
                bytes32(0),
                bridge.computeNextWithdrawState(bytes32(0), withdrawalRoot, 1),
                withdrawalRoot,
                1
            ),
            ""
        );

        bytes32[16] memory proof = _merkleProof(leaves, 0);
        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.claimWithdraw(
            keccak256("wrong old action state"),
            target,
            0,
            proof
        );
    }

    function test_SubmitWithdrawTransition_RevertsAboveMaxWithdrawCount()
        public
    {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);
        uint32 invalidWithdrawCount = uint32(bridge.MAX_WITHDRAW_COUNT() + 1);

        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                oldActionState,
                actionState,
                bytes32(0),
                keccak256("withdraw state"),
                keccak256("withdrawal root"),
                invalidWithdrawCount
            ),
            ""
        );
    }

    function test_SubmitWithdrawTransition_RevertsOnZeroRootForNonEmptyBatch()
        public
    {
        bytes32 oldActionState = keccak256("old action state");
        bytes32 actionState = keccak256("action state");
        settlement.setL2ActionStateInfo(oldActionState, 0, true);
        settlement.setL2ActionStateInfo(actionState, 1, true);

        vm.expectRevert(EthereumZekoBridge.InvalidWithdrawProof.selector);
        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                oldActionState,
                actionState,
                bytes32(0),
                keccak256("withdraw state"),
                bytes32(0),
                1
            ),
            ""
        );
    }

    function test_SubmitWithdrawTransition_AllowsSameRootForDifferentActionStates()
        public
    {
        bytes32 firstActionState = keccak256("first action state");
        bytes32 secondActionState = keccak256("second action state");
        bytes32 thirdActionState = keccak256("third action state");
        bytes32 withdrawalRoot = keccak256("withdrawal root");
        bytes32 firstWithdrawState = bridge.computeNextWithdrawState(
            bytes32(0),
            withdrawalRoot,
            1
        );
        bytes32 secondWithdrawState = bridge.computeNextWithdrawState(
            firstWithdrawState,
            withdrawalRoot,
            1
        );
        settlement.setL2ActionStateInfo(firstActionState, 0, true);
        settlement.setL2ActionStateInfo(secondActionState, 1, true);
        settlement.setL2ActionStateInfo(thirdActionState, 2, true);

        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                firstActionState,
                secondActionState,
                bytes32(0),
                firstWithdrawState,
                withdrawalRoot,
                1
            ),
            ""
        );

        bridge.submitWithdrawTransition(
            _withdrawPublicValues(
                secondActionState,
                thirdActionState,
                firstWithdrawState,
                secondWithdrawState,
                withdrawalRoot,
                1
            ),
            ""
        );

        (bytes32 firstStoredRoot, , , , , bool firstValid) = bridge
            .withdrawalRootInfo(firstActionState);
        (bytes32 secondStoredRoot, , , , , bool secondValid) = bridge
            .withdrawalRootInfo(secondActionState);
        assertEq(firstStoredRoot, withdrawalRoot);
        assertEq(secondStoredRoot, withdrawalRoot);
        assertTrue(firstValid);
        assertTrue(secondValid);
    }

    function test_DecodeWithdrawPublicValues_Expects164Bytes() public view {
        bytes32 withdrawalRoot = keccak256("withdrawal root");
        EthereumZekoBridge.DecodedWithdrawPublicValues memory decoded = bridge
            .decodeWithdrawPublicValues(
                _withdrawPublicValues(
                    bytes32(uint256(1)),
                    bytes32(uint256(2)),
                    bytes32(uint256(3)),
                    bytes32(uint256(4)),
                    withdrawalRoot,
                    5
                )
            );

        assertEq(decoded.withdrawalRoot, withdrawalRoot);
        assertEq(decoded.withdrawCount, 5);
    }

    function test_ComputeNextWithdrawState_MatchesSP1Fixture() public view {
        bytes32 withdrawalRoot = 0x662c8b3d64189c52eae01750e77211f293f6ea2a44d277afcf71044ad9926b9e;
        bytes32 expectedState = 0xbec5df338b7bc84f048893780b334917e56c7e90ca9b8fc926f0ec31da995ffc;

        assertEq(
            bridge.computeNextWithdrawState(bytes32(0), withdrawalRoot, 3),
            expectedState
        );
    }

    function test_DecodeWithdrawPublicValues_RevertsOnOld132Bytes() public {
        bytes memory oldPublicValues = new bytes(132);
        vm.expectRevert(
            abi.encodeWithSelector(
                EthereumZekoBridge.InvalidBridgePublicValuesLength.selector,
                uint256(164),
                uint256(132)
            )
        );
        bridge.decodeWithdrawPublicValues(oldPublicValues);
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
        publicValues = new bytes(148);
        uint256 cursor = 0;

        _writeBytes32(publicValues, cursor, ethereumStateBefore);
        cursor += 32;
        _writeBytes32(publicValues, cursor, ethereumStateAfter);
        cursor += 32;
        _writeUint64LE(publicValues, cursor, ethereumNonceBefore);
        cursor += 8;
        _writeUint64LE(publicValues, cursor, ethereumNonceAfter);
        cursor += 8;
        _writeBytes32(publicValues, cursor, zekoActionStateBefore);
        cursor += 32;
        _writeBytes32(publicValues, cursor, zekoActionStateAfter);
        cursor += 32;
        _writeUint32LE(publicValues, cursor, depositCount);
        cursor += 4;
        assert(cursor == publicValues.length);
    }

    function _withdrawPublicValues(
        bytes32 zekoActionStateBefore,
        bytes32 zekoActionStateAfter,
        bytes32 ethereumWithdrawStateBefore,
        bytes32 ethereumWithdrawStateAfter,
        uint32 withdrawCount
    ) private pure returns (bytes memory publicValues) {
        return
            _withdrawPublicValues(
                zekoActionStateBefore,
                zekoActionStateAfter,
                ethereumWithdrawStateBefore,
                ethereumWithdrawStateAfter,
                keccak256(
                    abi.encode(
                        "test withdrawal root",
                        ethereumWithdrawStateAfter
                    )
                ),
                withdrawCount
            );
    }

    function _withdrawPublicValues(
        bytes32 zekoActionStateBefore,
        bytes32 zekoActionStateAfter,
        bytes32 ethereumWithdrawStateBefore,
        bytes32 ethereumWithdrawStateAfter,
        bytes32 withdrawalRoot,
        uint32 withdrawCount
    ) private pure returns (bytes memory publicValues) {
        publicValues = new bytes(164);
        uint256 cursor = 0;

        _writeBytes32(publicValues, cursor, zekoActionStateBefore);
        cursor += 32;
        _writeBytes32(publicValues, cursor, zekoActionStateAfter);
        cursor += 32;
        _writeBytes32(publicValues, cursor, ethereumWithdrawStateBefore);
        cursor += 32;
        _writeBytes32(publicValues, cursor, ethereumWithdrawStateAfter);
        cursor += 32;
        _writeBytes32(publicValues, cursor, withdrawalRoot);
        cursor += 32;
        _writeUint32LE(publicValues, cursor, withdrawCount);
        cursor += 4;

        assert(cursor == publicValues.length);
    }

    function _merkleRoot(
        bytes32[] memory leaves
    ) private view returns (bytes32) {
        if (leaves.length == 0) return _zeroHashes()[16];

        bytes32[17] memory zeroHashes = _zeroHashes();
        bytes32[] memory nodes = leaves;

        for (uint256 level = 0; level < 16; level++) {
            bytes32[] memory parents = new bytes32[]((nodes.length + 1) / 2);
            for (uint256 i = 0; i < nodes.length; i += 2) {
                bytes32 right = i + 1 < nodes.length
                    ? nodes[i + 1]
                    : zeroHashes[level];
                parents[i / 2] = _hashMerkleNode(nodes[i], right);
            }
            nodes = parents;
        }

        return nodes[0];
    }

    function _merkleProof(
        bytes32[] memory leaves,
        uint256 targetIndex
    ) private view returns (bytes32[16] memory proof) {
        bytes32[17] memory zeroHashes = _zeroHashes();
        bytes32[] memory nodes = leaves;
        uint256 index = targetIndex;

        for (uint256 level = 0; level < 16; level++) {
            uint256 siblingIndex = index ^ 1;
            proof[level] = siblingIndex < nodes.length
                ? nodes[siblingIndex]
                : zeroHashes[level];

            bytes32[] memory parents = new bytes32[]((nodes.length + 1) / 2);
            for (uint256 i = 0; i < nodes.length; i += 2) {
                bytes32 right = i + 1 < nodes.length
                    ? nodes[i + 1]
                    : zeroHashes[level];
                parents[i / 2] = _hashMerkleNode(nodes[i], right);
            }
            nodes = parents;
            index >>= 1;
        }
    }

    function _zeroHashes() private view returns (bytes32[17] memory hashes) {
        for (uint256 level = 0; level < 16; level++) {
            hashes[level + 1] = _hashMerkleNode(hashes[level], hashes[level]);
        }
    }

    function _hashMerkleNode(
        bytes32 left,
        bytes32 right
    ) private view returns (bytes32) {
        return
            keccak256(
                abi.encode(bridge.WITHDRAW_MERKLE_NODE_DOMAIN(), left, right)
            );
    }

    function _singleInnerActionTree(
        bytes32 leaf
    ) private view returns (bytes32 root, bytes32[16] memory proof) {
        root = leaf;
        bytes32 zero;
        for (uint256 level = 0; level < 16; level++) {
            proof[level] = zero;
            root = keccak256(
                abi.encode(bridge.INNER_ACTION_NODE_V2_DOMAIN(), root, zero)
            );
            zero = keccak256(
                abi.encode(bridge.INNER_ACTION_NODE_V2_DOMAIN(), zero, zero)
            );
        }
    }

    function _twoAssetRecordBatch(
        bytes32 firstRecordHash,
        bytes32 secondRecordHash
    )
        private
        view
        returns (
            bytes32 root,
            bytes32[8] memory firstProof,
            bytes32[8] memory secondProof
        )
    {
        bytes32[256] memory nodes;
        nodes[0] = keccak256(
            abi.encodePacked(
                registryModule.ASSET_RECORD_BATCH_LEAF_V1_DOMAIN(),
                firstRecordHash
            )
        );
        nodes[1] = keccak256(
            abi.encodePacked(
                registryModule.ASSET_RECORD_BATCH_LEAF_V1_DOMAIN(),
                secondRecordHash
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
                    abi.encodePacked(
                        registryModule.ASSET_RECORD_BATCH_NODE_V1_DOMAIN(),
                        nodes[index],
                        nodes[index + 1]
                    )
                );
            }
            width >>= 1;
            firstIndex >>= 1;
            secondIndex >>= 1;
        }
        root = nodes[0];
    }

    function _writeBytes32(
        bytes memory data,
        uint256 offset,
        bytes32 value
    ) private pure {
        assembly {
            mstore(add(add(data, 0x20), offset), value)
        }
    }

    function _writeUint64LE(
        bytes memory data,
        uint256 offset,
        uint64 value
    ) private pure {
        for (uint256 i = 0; i < 8; i++) {
            data[offset + i] = bytes1(uint8(value >> (8 * i)));
        }
    }

    function _writeUint32LE(
        bytes memory data,
        uint256 offset,
        uint32 value
    ) private pure {
        for (uint256 i = 0; i < 4; i++) {
            data[offset + i] = bytes1(uint8(value >> (8 * i)));
        }
    }

    function _addressField(address value) private pure returns (bytes32) {
        return bytes32(uint256(uint160(value)));
    }
}
