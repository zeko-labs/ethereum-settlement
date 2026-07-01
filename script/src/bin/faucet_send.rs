use anyhow::{Context, Result};
use clap::Parser;
use ledger::scan_state::{
    currency::{Amount, Fee, Nonce},
    transaction_logic::{
        signed_command::{Body, PaymentPayload, SignedCommandPayload},
        transaction_union_payload::TransactionUnionPayload,
        Memo,
    },
};
use mina_signer::{CompressedPubKey, Keypair, NetworkId, SecKey, Signer};
use serde_json::json;
use std::str::FromStr;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    faucet_sk: String,
    #[arg(long)]
    to: String,
    #[arg(long, default_value = "1000000000000")]
    amount: u64,
    #[arg(long, default_value = "1000000")]
    fee: u64,
    #[arg(long, default_value = "0")]
    nonce: u32,
    #[arg(long)]
    memo: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let secret = SecKey::from_base58(args.faucet_sk.trim()).context("invalid faucet private key")?;
    let keypair = Keypair::from_secret_key(secret).context("derive faucet keypair")?;
    let from = keypair.public.into_compressed();
    let to = CompressedPubKey::from_address(args.to.trim()).context("invalid recipient address")?;

    let memo = match args.memo {
        Some(memo) => Memo::from_str(&memo).map_err(|_| anyhow::anyhow!("invalid memo"))?,
        None => Memo::empty(),
    };

    let payload = SignedCommandPayload::create(
        Fee::from_u64(args.fee),
        from.clone(),
        Nonce::from_u32(args.nonce),
        None,
        memo,
        Body::Payment(PaymentPayload {
            receiver_pk: to.clone(),
            amount: Amount::from_u64(args.amount),
        }),
    );

    let tx = TransactionUnionPayload::of_user_command_payload(&payload);
    let mut signer = mina_signer::create_legacy(NetworkId::TESTNET);
    let signature = signer.sign(&keypair, &tx, false);

    let query = r#"
mutation SendPayment($signature: SignatureInput, $input: SendPaymentInput!) {
  sendPayment(signature: $signature, input: $input) {
    payment {
      id
      hash
    }
  }
}
"#;

    let body = json!({
        "query": query,
        "variables": {
            "signature": {
                "field": signature.rx.to_string(),
                "scalar": signature.s.to_string()
            },
            "input": {
                "from": from.into_address(),
                "to": to.into_address(),
                "amount": args.amount.to_string(),
                "fee": args.fee.to_string(),
                "nonce": args.nonce.to_string()
            }
        }
    });

    println!("{}", serde_json::to_string(&body)?);
    Ok(())
}
