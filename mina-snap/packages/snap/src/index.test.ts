import { describe, expect, it } from "@jest/globals"
import {
  assertIsConfirmationDialog,
  installSnap
} from "@metamask/snaps-jest"
import { auroBerkeleyFixture } from "./fixtures/auro-berkeley-zkapp"

describe("Auro-compatible Mina Snap RPC", () => {
  const connect = async (
    request: Awaited<ReturnType<typeof installSnap>>["request"],
    origin = "https://bridge.zeko.io"
  ) => {
    const pending = request({ origin, method: "mina_requestAccounts" })
    const dialog = await pending.getInterface()
    assertIsConfirmationDialog(dialog)
    await dialog.ok()
    return pending
  }

  const approve = async (pending: ReturnType<Awaited<ReturnType<typeof installSnap>>["request"]>) => {
    const dialog = await pending.getInterface()
    assertIsConfirmationDialog(dialog)
    await dialog.ok()
    return pending
  }

  it("recovers Auro account zero from MetaMask's recovery phrase", async () => {
    const { request } = await installSnap()

    const response = await connect(request)

    expect(await response).toRespondWith([
      "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA"
    ])
    expect(await request({
      origin: "https://bridge.zeko.io",
      method: "mina_accounts"
    })).toRespondWith([
      "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA"
    ])
    expect(await request({
      origin: "https://unapproved.example",
      method: "mina_accounts"
    })).toRespondWith([])
    expect(await request({
      origin: "https://bridge.zeko.io",
      method: "wallet_revokePermissions"
    })).toRespondWith([])
    expect(await request({
      origin: "https://bridge.zeko.io",
      method: "mina_accounts"
    })).toRespondWith([])
  })

  it("signs a message exactly like Auro after user approval", async () => {
    const { request } = await installSnap()
    await connect(request)
    expect(await approve(request({
      origin: "https://bridge.zeko.io",
      method: "mina_switchChain",
      params: { networkID: "zeko:testnet" }
    }))).toRespondWith({ networkID: "zeko:testnet" })
    const pending = request({
      origin: "https://bridge.zeko.io",
      method: "mina_signMessage",
      params: { message: "Zeko bridge compatibility" }
    })

    const dialog = await pending.getInterface()
    assertIsConfirmationDialog(dialog)
    await dialog.ok()

    expect(await pending).toRespondWith({
      signature: {
        field: "17227017063632854446394812775928915936565282157552302831098756722799259545054",
        scalar: "3313393638225204816881738803494716850768532821405075789516348239933350706297"
      },
      publicKey: "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA",
      data: "Zeko bridge compatibility"
    })
  })

  it("signs and verifies fields in Auro's provider format", async () => {
    const { request } = await installSnap()
    await connect(request)
    const pending = request({
      origin: "https://bridge.zeko.io",
      method: "mina_signFields",
      params: { message: ["1", 2] }
    })
    const dialog = await pending.getInterface()
    assertIsConfirmationDialog(dialog)
    await dialog.ok()

    const signed = {
      data: ["1", 2],
      publicKey: "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA",
      signature: "7mXGpbUnCibuAR7f8VTUtw1AomQ2kPXNDgre9E7xxo5kjfZ53f9Wbvn32pdKqGo3zTFhMRhVaMC3sGBw46RR375Cmtf4tLi3"
    }
    expect(await pending).toRespondWith(signed)
    expect(await request({
      method: "mina_verifyFields",
      params: signed
    })).toRespondWith(true)
  })

  it("returns Auro's signedData envelope for onlySign zkApp transactions", async () => {
    const { request } = await installSnap()
    await connect(request)
    await approve(request({
      origin: "https://bridge.zeko.io",
      method: "mina_switchChain",
      params: { networkID: "zeko:testnet" }
    }))
    const transaction = {
      feePayer: {
        body: {
          publicKey: "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA",
          fee: "100000000",
          validUntil: null,
          nonce: "0"
        },
        authorization: "7mWxjLYgbJUkZNcGouvhVj5tJ8yu9hoexb9ntvPK8t5LHqzmrL6QJjjKtf5SgmxB4QWkDw7qoMMbbNGtHVpsbJHPyTy2EzRQ"
      },
      accountUpdates: [],
      memo: "E4YM2vTHhWEg66xpj52JErHUBU4pZ1yageL4TVDDpTTSsv8mK6YaH"
    }
    const pending = request({
      origin: "https://bridge.zeko.io",
      method: "mina_sendTransaction",
      params: { onlySign: true, transaction: JSON.stringify(transaction) }
    })
    await approve(pending)
    const result = await pending
    if (!("result" in result.response)) {
      throw new Error(`Snap returned ${JSON.stringify(result.response.error)}`)
    }
    const signedData = JSON.parse(
      (result.response.result as { signedData: string }).signedData
    ) as { zkappCommand: typeof transaction }
    expect(signedData.zkappCommand.feePayer.authorization).toBe(
      "7mX4XaHcKcXbtwhTi6cHns6JWYWBSpF3Wz11D1abs9txNVV5pemhaRAPUbvhaBiVHm8SHPyhZTBYWKR8cdiJvf5us8s8PAn1"
    )
  })

  it("applies Auro's fee, nonce, and memo overrides before signing a zkApp", async () => {
    const { request } = await installSnap()
    await connect(request)
    await approve(request({
      origin: "https://bridge.zeko.io",
      method: "mina_switchChain",
      params: { networkID: "zeko:testnet" }
    }))
    const transaction = {
      feePayer: {
        body: {
          publicKey: "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA",
          fee: "100000000",
          validUntil: null,
          nonce: "0"
        },
        authorization: ""
      },
      accountUpdates: [],
      memo: "E4YM2vTHhWEg66xpj52JErHUBU4pZ1yageL4TVDDpTTSsv8mK6YaH"
    }
    const pending = request({
      origin: "https://bridge.zeko.io",
      method: "mina_sendTransaction",
      params: {
        onlySign: true,
        nonce: 3,
        feePayer: { fee: 0.2, memo: "Zeko bridge" },
        transaction: JSON.stringify(transaction)
      }
    })
    await approve(pending)
    const result = await pending
    if (!("result" in result.response)) {
      throw new Error(`Snap returned ${JSON.stringify(result.response.error)}`)
    }
    const signed = JSON.parse(
      (result.response.result as { signedData: string }).signedData
    ) as { zkappCommand: typeof transaction }

    expect(signed.zkappCommand.feePayer.body).toMatchObject({
      fee: "200000000",
      nonce: "3"
    })
    expect(signed.zkappCommand.memo).not.toBe(transaction.memo)
  })

  it("matches Auro's Berkeley-era signature for a bridge-shaped zkApp", async () => {
    const { request } = await installSnap()
    await connect(request)
    await approve(request({
      origin: "https://bridge.zeko.io",
      method: "mina_switchChain",
      params: { networkID: "zeko:testnet" }
    }))
    const pending = request({
      origin: "https://bridge.zeko.io",
      method: "mina_sendTransaction",
      params: {
        onlySign: true,
        transaction: JSON.stringify(auroBerkeleyFixture.transaction)
      }
    })
    await approve(pending)
    const result = await pending
    if (!("result" in result.response)) {
      throw new Error(`Snap returned ${JSON.stringify(result.response.error)}`)
    }
    const signed = JSON.parse(
      (result.response.result as { signedData: string }).signedData
    ) as { zkappCommand: { feePayer: { authorization: string } } }
    expect(signed.zkappCommand.feePayer.authorization).toBe(
      auroBerkeleyFixture.expectedFeePayerAuthorization
    )
  })

  it("signs and verifies Auro JSON messages", async () => {
    const { request } = await installSnap()
    await connect(request)
    await approve(request({
      origin: "https://bridge.zeko.io",
      method: "mina_switchChain",
      params: { networkID: "zeko:testnet" }
    }))
    const pending = request({
      origin: "https://bridge.zeko.io",
      method: "mina_sign_JsonMessage",
      params: { message: [{ label: "Bridge", value: "Zeko" }] }
    })
    await approve(pending)
    const signed = {
      signature: {
        field: "21837675687846711461885219514611916957507484202255622234238839060111617835431",
        scalar: "4605570600677247238516219628611358080943999401922708775502897719692047972097"
      },
      publicKey: "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA",
      data: "[{\"label\":\"Bridge\",\"value\":\"Zeko\"}]"
    }
    expect(await pending).toRespondWith(signed)
    expect(await request({
      method: "mina_verify_JsonMessage",
      params: signed
    })).toRespondWith(true)
  })

  it("creates Auro-compatible nullifiers without leaking non-JSON bigints", async () => {
    const { request } = await installSnap()
    await connect(request)
    const pending = request({
      origin: "https://bridge.zeko.io",
      method: "mina_createNullifier",
      params: { message: ["1", 2] }
    })
    await approve(pending)
    expect(await pending).toRespondWith(expect.objectContaining({
      publicKey: {
        x: "15183282546212648667772401125503300032473030605628576573844682624119605053887",
        y: "1761624623241401639545529325794604234197437066122748801627185328273940366044"
      },
      public: expect.objectContaining({
        nullifier: {
          x: "20939173034483011605129258232144037878732510845220323027250518803416023908017",
          y: "852139252747709757403899457771078659661150434711401573101640737656260650376"
        }
      })
    }))
  })

  it("asks before contacting a dapp-supplied Mina GraphQL endpoint", async () => {
    const { request } = await installSnap()
    await connect(request)
    const pending = request({
      origin: "https://bridge.zeko.io",
      method: "mina_addChain",
      params: {
        name: "Untrusted endpoint",
        url: "http://127.0.0.1:1/graphql"
      }
    })

    const dialog = await pending.getInterface()
    assertIsConfirmationDialog(dialog)
    await dialog.cancel()

    const response = await pending
    expect(response.response).toMatchObject({ error: { code: 4001 } })
  })

  it("returns standard authorization and parameter errors", async () => {
    const { request } = await installSnap()

    const unauthorized = await request({
      origin: "https://bridge.zeko.io",
      method: "mina_signMessage",
      params: { message: "not authorized" }
    })
    expect(unauthorized.response).toMatchObject({
      error: {
        code: 4100,
        message: "Connect the Mina account before signing"
      }
    })

    await connect(request)
    const invalid = await request({
      origin: "https://bridge.zeko.io",
      method: "mina_signMessage",
      params: { message: 7 }
    })
    expect(invalid.response).toMatchObject({
      error: {
        code: -32602,
        message: "mina_signMessage requires a string message"
      }
    })

    const invalidPayment = await request({
      origin: "https://bridge.zeko.io",
      method: "mina_sendPayment",
      params: {
        to: "not-a-mina-address",
        amount: 1,
        nonce: -1
      }
    })
    expect(invalidPayment.response).toMatchObject({
      error: { code: -32602 }
    })
  })
})
