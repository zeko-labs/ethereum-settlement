// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

import {EthereumZekoBridge} from "../src/EthereumZekoBridge.sol";

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

contract EthereumZekoBridgeTest is Test {
    EthereumZekoBridge internal bridge;
    TestERC20 internal token18;
    TestERC20 internal token6;

    address internal owner = address(this);
    address internal alice = address(0xA11CE);

    function setUp() public {
        bridge = new EthereumZekoBridge(owner);
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
        vm.prank(alice);
        vm.expectRevert();
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

        vm.prank(alice);
        vm.expectRevert();
        bridge.setTokenAllowed(address(token18), false);
    }

    function test_Deposit_SerializesBridgeAddressAndNormalizedAmount() public {
        bridge.addToken(address(token18), true, 9, 18);

        uint256 amount = 2 ether;
        uint64 timeout = 123456;
        bytes memory recipient = hex"01020304";

        vm.startPrank(alice);
        token18.approve(address(bridge), amount);
        (uint64 nonce, bytes32 leaf, bytes32 newState) = bridge.deposit(
            address(token18),
            amount,
            recipient,
            timeout
        );
        vm.stopPrank();

        bytes32 recipientHash = keccak256(recipient);
        bytes32 expectedLeaf = keccak256(
            abi.encode(
                bridge.DEPOSIT_LEAF_DOMAIN(),
                block.chainid,
                address(bridge),
                address(token18),
                recipientHash,
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

    function test_DepositETH_UsesNativeTokenConfig() public {
        uint256 amount = 3 ether;
        uint64 timeout = 777;
        bytes memory recipient = hex"deadbeef";

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
                keccak256(recipient),
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

    function test_DepositETH_RevertsWhenPrecisionDoesNotFitZekoDecimals() public {
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
        bridge.depositETH{value: 1 ether + 1}(hex"01", 1);
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
        bridge.deposit(address(token18), 1 ether + 1, hex"01", 99);
        vm.stopPrank();
    }

    function test_Deposit_ScalesUpWhenEthereumHasFewerDecimals() public {
        bridge.addToken(address(token6), true, 9, 6);

        uint256 amount = 25 * 10 ** 6;
        uint64 timeout = 88;
        bytes memory recipient = hex"1234";

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
                keccak256(recipient),
                25 * 10 ** 9,
                timeout,
                uint64(1)
            )
        );

        assertEq(leaf, expectedLeaf);
    }
}
