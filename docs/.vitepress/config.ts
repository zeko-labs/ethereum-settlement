import { defineConfig } from "vitepress"

export default defineConfig({
  title: "Zeko on Ethereum",
  description: "Architecture and operations for the multisig-DA Ethereum settlement PoC",
  srcDir: "content",
  cleanUrls: true,
  lastUpdated: true,
  head: [
    ["link", { rel: "icon", type: "image/svg+xml", href: "/favicon.svg" }],
    ["link", { rel: "shortcut icon", href: "/favicon.ico" }],
    ["link", { rel: "apple-touch-icon", sizes: "180x180", href: "/apple-touch-icon.png" }],
    ["meta", { property: "og:title", content: "Zeko on Ethereum" }],
    ["meta", { property: "og:description", content: "Multisig-DA Ethereum settlement PoC" }],
    ["meta", { property: "og:image", content: "/og-image.png" }]
  ],
  themeConfig: {
    search: {
      provider: "local",
      options: { detailedView: true }
    },
    nav: [
      { text: "Docs", link: "/overview" },
      { text: "Testnet runbook", link: "/operations/testnet" },
      { text: "Zeko Docs", link: "https://docs.zeko.io", target: "_blank" }
    ],
    logo: { light: "/logo.svg", dark: "/logo-dark.svg" },
    sidebar: [
      {
        text: "Introduction",
        items: [
          { text: "Overview", link: "/overview" },
          { text: "Current status", link: "/status" },
          { text: "Architecture", link: "/architecture" }
        ]
      },
      {
        text: "Protocol flows",
        items: [
          { text: "Settlement", link: "/protocol/settlement" },
          { text: "Native deposits", link: "/protocol/deposit-bridge" },
          { text: "Native withdrawals", link: "/protocol/withdrawals" }
        ]
      },
      {
        text: "Gateway",
        items: [
          { text: "API and GraphQL", link: "/gateway/api" },
          { text: "Proof jobs and approval", link: "/gateway/proving" },
          { text: "Bridge web application", link: "/bridge-ui" }
        ]
      },
      {
        text: "Operations",
        items: [
          { text: "Local E2E", link: "/operations/local-e2e" },
          { text: "Sepolia testnet", link: "/operations/testnet" },
          { text: "DevOps and NixOS", link: "/operations/devops" }
        ]
      },
      {
        text: "Development",
        items: [
          { text: "Toolchains", link: "/development/toolchains" },
          { text: "Verification", link: "/development/verification" }
        ]
      },
      {
        text: "Reference",
        items: [
          { text: "Configuration", link: "/reference/configuration" },
          { text: "Security model", link: "/reference/security-model" },
          { text: "Command reference", link: "/reference/commands" }
        ]
      }
    ],
    socialLinks: [
      { icon: "github", link: "https://github.com/zeko-labs/ethereum-settlement" }
    ],
    editLink: {
      pattern: "https://github.com/zeko-labs/ethereum-settlement/edit/main/docs/content/:path",
      text: "Edit this page on GitHub"
    },
    footer: {
      message: "Experimental Zeko settlement and native bridge glue for Ethereum.",
      copyright: "Copyright © 2026 Zeko Labs"
    }
  }
})
