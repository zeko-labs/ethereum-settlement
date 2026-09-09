export const auroBerkeleyFixture = {
  "source": {
    "wallet": "Auro Wallet 2.5.2",
    "commit": "65db631c1726ea640df6f2f73275347cc0fc275f",
    "signer": "mina-signer 4.1.0",
    "path": "m/44'/12586'/0'/0/0"
  },
  "transaction": {
    "feePayer": {
      "body": {
        "publicKey": "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA",
        "fee": "100000000",
        "validUntil": null,
        "nonce": "0"
      },
      "authorization": "7mWxjLYgbJUkZNcGouvhVj5tJ8yu9hoexb9ntvPK8t5LHqzmrL6QJjjKtf5SgmxB4QWkDw7qoMMbbNGtHVpsbJHPyTy2EzRQ"
    },
    "accountUpdates": [
      {
        "body": {
          "publicKey": "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA",
          "tokenId": "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf",
          "update": {
            "appState": [
              null,
              null,
              null,
              null,
              null,
              null,
              null,
              null
            ],
            "delegate": null,
            "verificationKey": null,
            "permissions": null,
            "zkappUri": null,
            "tokenSymbol": null,
            "timing": null,
            "votingFor": null
          },
          "balanceChange": {
            "magnitude": "1",
            "sgn": "Negative"
          },
          "incrementNonce": false,
          "events": [],
          "actions": [],
          "callData": "0",
          "callDepth": 0,
          "preconditions": {
            "network": {
              "snarkedLedgerHash": null,
              "blockchainLength": null,
              "minWindowDensity": null,
              "totalCurrency": null,
              "globalSlotSinceGenesis": null,
              "stakingEpochData": {
                "ledger": {
                  "hash": null,
                  "totalCurrency": null
                },
                "seed": null,
                "startCheckpoint": null,
                "lockCheckpoint": null,
                "epochLength": null
              },
              "nextEpochData": {
                "ledger": {
                  "hash": null,
                  "totalCurrency": null
                },
                "seed": null,
                "startCheckpoint": null,
                "lockCheckpoint": null,
                "epochLength": null
              }
            },
            "account": {
              "balance": null,
              "nonce": null,
              "receiptChainHash": null,
              "delegate": null,
              "state": [
                null,
                null,
                null,
                null,
                null,
                null,
                null,
                null
              ],
              "actionState": null,
              "provedState": null,
              "isNew": null
            },
            "validWhile": null
          },
          "useFullCommitment": true,
          "implicitAccountCreationFee": false,
          "mayUseToken": {
            "parentsOwnToken": false,
            "inheritFromParent": false
          },
          "authorizationKind": {
            "isSigned": true,
            "isProved": false,
            "verificationKeyHash": "3392518251768960475377392625298437850623664973002200885669375116181514017494"
          }
        },
        "authorization": {
          "proof": null,
          "signature": null
        }
      }
    ],
    "memo": "E4YM2vTHhWEg66xpj52JErHUBU4pZ1yageL4TVDDpTTSsv8mK6YaH"
  },
  "expectedFeePayerAuthorization": "7mXXPevdPuPvLMHTNimcGGu31cQqdyP9QJhLkRNMX78oftTWuJR9CFHpD4NEfobGaXwQKdQYGLtMYj8LS9KfTkd6jwkRLLSP"
} as const
