import { describe, expect, it } from "@jest/globals"
import {
  assertIsConfirmationDialog,
  installSnap
} from "@metamask/snaps-jest"
import { auroBerkeleyFixture } from "./fixtures/auro-berkeley-zkapp"
import {
  missingEndpointMessage,
  renderWalletHome,
  renderWalletLoading
} from "./home"

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

  const interfaceValues = (value: unknown): string[] => {
    if (typeof value === "string") return [value]
    if (Array.isArray(value)) return value.flatMap(interfaceValues)
    if (typeof value !== "object" || value === null) return []
    return Object.values(value).flatMap(interfaceValues)
  }

  it("renders a wallet home page and refreshes it interactively", async () => {
    const { onHomePage } = await installSnap()
    const response = await onHomePage()
    const page = response.getInterface()
    const expected = renderWalletHome({
      publicKey: "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA",
      networkId: "mina:mainnet",
      networkName: "Mina Mainnet",
      tokens: [],
      error: missingEndpointMessage("mina:mainnet")
    })

    expect(page).toRender(expected)

    const updated = page.waitForUpdate()
    await page.clickElement("refresh-balances")
    expect(await updated).toRender(renderWalletLoading({
      publicKey: "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA",
      networkName: "Mina Mainnet"
    }))
  })

  it("does not trust a hard-coded Zeko testnet data source", async () => {
    const { request, onHomePage } = await installSnap()
    await connect(request)
    await approve(request({
      origin: "https://bridge.zeko.io",
      method: "mina_switchChain",
      params: { networkID: "zeko:testnet" }
    }))

    expect((await onHomePage()).getInterface()).toRender(renderWalletHome({
      publicKey: "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA",
      networkId: "zeko:testnet",
      networkName: "Zeko Testnet",
      tokens: [],
      error: missingEndpointMessage("zeko:testnet")
    }))
  })

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

  it("preserves a non-empty command memo when no override is supplied", async () => {
    const { request } = await installSnap()
    await connect(request)
    await approve(request({
      origin: "https://bridge.zeko.io",
      method: "mina_switchChain",
      params: { networkID: "zeko:testnet" }
    }))
    const transaction = JSON.parse(JSON.stringify(auroBerkeleyFixture.transaction)) as {
      feePayer: { body: { validUntil: string | null } }
      memo: string
      [key: string]: unknown
    }
    transaction.feePayer.body.validUntil = "42"
    const withMemo = request({
      origin: "https://bridge.zeko.io",
      method: "mina_sendTransaction",
      params: {
        onlySign: true,
        feePayer: { memo: "Keep this memo" },
        transaction: JSON.stringify(transaction)
      }
    })
    await approve(withMemo)
    const first = await withMemo
    if (!("result" in first.response)) {
      throw new Error(`Snap returned ${JSON.stringify(first.response.error)}`)
    }
    const memo = (JSON.parse(
      (first.response.result as { signedData: string }).signedData
    ) as { zkappCommand: { memo: string } }).zkappCommand.memo
    const preservedTransaction = { ...transaction, memo }

    const pending = request({
      origin: "https://bridge.zeko.io",
      method: "mina_sendTransaction",
      params: {
        onlySign: true,
        transaction: JSON.stringify(preservedTransaction)
      }
    })
    await approve(pending)
    const result = await pending
    if (!("result" in result.response)) {
      throw new Error(`Snap returned ${JSON.stringify(result.response.error)}`)
    }
    const signed = JSON.parse(
      (result.response.result as { signedData: string }).signedData
    ) as { zkappCommand: { feePayer: { body: { validUntil: string | null } }; memo: string } }

    expect(signed.zkappCommand.memo).toBe(memo)
    expect(signed.zkappCommand.feePayer.body.validUntil).toBe("42")
  })

  it("shows every signed zkApp update and a canonical payload hash", async () => {
    const { request } = await installSnap()
    await connect(request)
    await approve(request({
      origin: "https://bridge.zeko.io",
      method: "mina_switchChain",
      params: { networkID: "zeko:testnet" }
    }))
    const transaction = JSON.parse(JSON.stringify(auroBerkeleyFixture.transaction)) as {
      accountUpdates: Array<{ body: { actions: string[][]; events: string[][]; callData: string } }>
    }
    const update = transaction.accountUpdates[0]?.body
    if (!update) throw new Error("Fixture has no account update")
    update.actions = [["11", "12"]]
    update.events = [["21", "22"]]
    update.callData = "314159"
    const pending = request({
      origin: "https://bridge.zeko.io",
      method: "mina_sendTransaction",
      params: {
        onlySign: true,
        transaction: JSON.stringify(transaction)
      }
    })
    const dialog = await pending.getInterface()
    assertIsConfirmationDialog(dialog)
    const values = interfaceValues(dialog.content)

    expect(values).toContain("B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA")
    expect(values).toContain("wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf")
    expect(values).toContain("Negative 1")
    expect(values).toContain("[[\"11\",\"12\"]]")
    expect(values).toContain("[[\"21\",\"22\"]]")
    expect(values).toContain("314159")
    expect(values).toContain("Signature")
    expect(values).toContain("This Snap will sign this update")
    expect(values).toContain("Canonical payload SHA-256")
    expect(values.some((value) => /^[0-9a-f]{64}$/u.test(value))).toBe(true)
    await dialog.cancel()
  })

  it("binds sign-only versus sign-and-submit into zkApp approval", async () => {
    const { request } = await installSnap()
    await connect(request)
    const transaction = JSON.parse(JSON.stringify(auroBerkeleyFixture.transaction))

    const signOnly = request({
      origin: "https://bridge.zeko.io",
      method: "mina_sendTransaction",
      params: { onlySign: true, transaction: JSON.stringify(transaction) }
    })
    const signOnlyDialog = await signOnly.getInterface()
    assertIsConfirmationDialog(signOnlyDialog)
    const signOnlyValues = interfaceValues(signOnlyDialog.content)
    const signOnlyHash = signOnlyValues.find((value) => /^[0-9a-f]{64}$/u.test(value))
    expect(signOnlyValues).toContain("Sign only")
    expect(signOnlyHash).toBeDefined()
    await signOnlyDialog.cancel()

    const signAndSubmit = request({
      origin: "https://bridge.zeko.io",
      method: "mina_sendTransaction",
      params: { onlySign: false, transaction: JSON.stringify(transaction) }
    })
    const signAndSubmitDialog = await signAndSubmit.getInterface()
    assertIsConfirmationDialog(signAndSubmitDialog)
    const signAndSubmitValues = interfaceValues(signAndSubmitDialog.content)
    const signAndSubmitHash = signAndSubmitValues.find((value) => /^[0-9a-f]{64}$/u.test(value))
    expect(signAndSubmitValues).toContain("Sign and submit")
    expect(signAndSubmitHash).toBeDefined()
    expect(signAndSubmitHash).not.toBe(signOnlyHash)
    await signAndSubmitDialog.cancel()
  })

  it("shows the exact normalized payment payload before signing", async () => {
    const { request } = await installSnap()
    await connect(request)
    const recipient = "B62qm7w14uvoXCU6LCTLnZnMT41qD2prFJEpYtRdU1Ny7BvgHcxhVT8"
    const pending = request({
      origin: "https://bridge.zeko.io",
      method: "mina_sendPayment",
      params: {
        to: recipient,
        amount: "1.25",
        fee: "0.02",
        nonce: 7,
        memo: "Bridge payment"
      }
    })
    const dialog = await pending.getInterface()
    assertIsConfirmationDialog(dialog)
    const values = interfaceValues(dialog.content)

    expect(values).toEqual(expect.arrayContaining([
      "Payment (sign and submit)",
      "Mina Mainnet (mina:mainnet)",
      "mainnet",
      "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA",
      "Recipient",
      recipient,
      "1250000000 nanomina",
      "20000000 nanomina",
      "7",
      "Bridge payment"
    ]))
    await dialog.cancel()
  })

  it("shows the exact normalized delegation payload before signing", async () => {
    const { request } = await installSnap()
    await connect(request)
    const delegate = "B62qm7w14uvoXCU6LCTLnZnMT41qD2prFJEpYtRdU1Ny7BvgHcxhVT8"
    const pending = request({
      origin: "https://bridge.zeko.io",
      method: "mina_sendStakeDelegation",
      params: {
        to: delegate,
        fee: "0.03",
        nonce: 9,
        memo: "Delegate stake"
      }
    })
    const dialog = await pending.getInterface()
    assertIsConfirmationDialog(dialog)
    const values = interfaceValues(dialog.content)

    expect(values).toEqual(expect.arrayContaining([
      "Stake delegation (sign and submit)",
      "Mina Mainnet (mina:mainnet)",
      "mainnet",
      "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA",
      "Delegate",
      delegate,
      "30000000 nanomina",
      "9",
      "Delegate stake"
    ]))
    await dialog.cancel()
  })

  it("rejects decimal quantities that exceed Mina UInt64 without expanding them", async () => {
    const { request } = await installSnap()
    await connect(request)
    const recipient = "B62qm7w14uvoXCU6LCTLnZnMT41qD2prFJEpYtRdU1Ny7BvgHcxhVT8"

    for (const amount of ["1e100000000", "18446744073.709551616"]) {
      const response = await request({
        origin: "https://bridge.zeko.io",
        method: "mina_sendPayment",
        params: { to: recipient, amount, fee: "0.1", nonce: 0 }
      })
      expect(response.response).toMatchObject({
        error: { code: -32602, message: "Amount exceeds Mina UInt64" }
      })
    }
  })

  it("partially signs a bridge zkApp whose fee payer is the sequencer", async () => {
    const { request } = await installSnap()
    await connect(request)
    await approve(request({
      origin: "https://bridge.zeko.io",
      method: "mina_switchChain",
      params: { networkID: "zeko:testnet" }
    }))
    const transaction = JSON.parse(JSON.stringify(
      auroBerkeleyFixture.transaction
    )) as {
      feePayer: { body: { publicKey: string }; authorization: string }
      accountUpdates: Array<{ authorization: { signature: string | null } }>
    }
    transaction.feePayer.body.publicKey =
      "B62qm7w14uvoXCU6LCTLnZnMT41qD2prFJEpYtRdU1Ny7BvgHcxhVT8"
    transaction.feePayer.authorization = ""

    const pending = request({
      origin: "https://bridge.zeko.io",
      method: "mina_sendTransaction",
      params: {
        onlySign: true,
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
    ) as {
      zkappCommand: {
        feePayer: { body: { publicKey: string }; authorization: string }
        accountUpdates: Array<{ authorization: { signature?: string } }>
      }
    }

    expect(signed.zkappCommand.feePayer).toEqual({
      body: expect.objectContaining({
        publicKey: transaction.feePayer.body.publicKey
      }),
      authorization: ""
    })
    expect(signed.zkappCommand.accountUpdates[0]?.authorization.signature)
      .toEqual(expect.stringMatching(/^7m/u))
  })

  it("rejects partial signing when the connected account is not a signer", async () => {
    const { request } = await installSnap()
    await connect(request)
    await approve(request({
      origin: "https://bridge.zeko.io",
      method: "mina_switchChain",
      params: { networkID: "zeko:testnet" }
    }))
    const transaction = JSON.parse(JSON.stringify(
      auroBerkeleyFixture.transaction
    )) as {
      feePayer: { body: { publicKey: string }; authorization: string }
      accountUpdates: Array<{ body: { publicKey: string } }>
    }
    transaction.feePayer.body.publicKey =
      "B62qm7w14uvoXCU6LCTLnZnMT41qD2prFJEpYtRdU1Ny7BvgHcxhVT8"
    transaction.feePayer.authorization = ""
    if (transaction.accountUpdates[0]) {
      transaction.accountUpdates[0].body.publicKey =
        "B62qo2SrsRijjVchPKVccPHVzi56u6Uu9zJYTzuVuNqhb27cMvCBaxe"
    }

    const response = await request({
      origin: "https://bridge.zeko.io",
      method: "mina_sendTransaction",
      params: {
        onlySign: true,
        transaction: JSON.stringify(transaction)
      }
    })

    expect(response.response).toMatchObject({
      error: {
        code: -32602,
        message: "The zkApp transaction does not request a signature from the connected account"
      }
    })
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
