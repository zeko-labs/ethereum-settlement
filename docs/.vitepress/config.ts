import { defineConfig } from "vitepress"

export default defineConfig({
  title: "Zeko Ethereum L2",
  description: "Proof-powered settlement and bridging between Zeko and Ethereum",
  srcDir: "content",
  cleanUrls: true,
  head: [
    ["link", { rel: "icon", type: "image/svg+xml", href: "/favicon.svg" }],
    ["link", { rel: "shortcut icon", href: "/favicon.ico" }],
    ["link", { rel: "apple-touch-icon", sizes: "180x180", href: "/apple-touch-icon.png" }],
    ["meta", { property: "og:title", content: "Zeko Ethereum L2" }],
    ["meta", { property: "og:description", content: "Proof-powered settlement and bridging between Zeko and Ethereum" }],
    ["meta", { property: "og:image", content: "/og-image.png" }]
  ],
  themeConfig: {
    search: {
      provider: "local",
      options: { detailedView: true }
    },
    nav: [
      { text: "Docs", link: "/overview" },
      { text: "Zeko Docs", link: "https://docs.zeko.io", target: "_blank" },
      { text: "Website", link: "https://zeko.io", target: "_blank" }
    ],
    logo: { light: "/logo.svg", dark: "/logo-dark.svg" },
    sidebar: [
      {
        text: "Introduction",
        items: [
          { text: "Overview", link: "/overview" },
          { text: "Architecture", link: "/architecture" }
        ]
      },
      {
        text: "Protocol Flows",
        items: [
          { text: "Settlement", link: "/protocol/settlement" },
          { text: "Deposit Bridge", link: "/protocol/deposit-bridge" },
          { text: "Withdrawals", link: "/protocol/withdrawals" }
        ]
      },
      {
        text: "Reference",
        items: [
          { text: "Security Model", link: "/reference/security-model" },
          { text: "Commands", link: "/reference/commands" },
          { text: "Cloudflare Deployment", link: "/reference/cloudflare" }
        ]
      }
    ],
    socialLinks: [
      { icon: "github", link: "https://github.com/zeko-labs/sp1-verifier" }
    ],
    editLink: {
      pattern: "https://github.com/zeko-labs/sp1-verifier/edit/bridge/docs/content/:path",
      text: "Edit this page on GitHub"
    },
    footer: {
      message: "Proof-powered settlement and bridging between Zeko and Ethereum.",
      copyright: "Copyright © 2026 Zeko Labs"
    }
  }
})
