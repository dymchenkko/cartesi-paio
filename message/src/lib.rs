use std::collections::HashMap;

use alloy_core::{
    primitives::{Address, SignatureError, U256},
    sol,
    sol_types::{eip712_domain, Eip712Domain, SolStruct},
};
use alloy_signer::Signature;

use serde::{Deserialize, Serialize};

pub struct WalletState {
    pub domain: Eip712Domain,

    // app address to app state
    pub app_nonces: HashMap<Address, AppNonces>,

    // user address to balance
    pub balances: HashMap<Address, U256>,
}

impl WalletState {
    pub fn verify_batch(&mut self, raw_batch: &[u8]) -> postcard::Result<Vec<Transaction>> {
        let batch: Batch = postcard::from_bytes(raw_batch)?;

        Ok(batch
            .txs
            .iter()
            .filter_map(|tx| {
                let app_nonce = self.app_nonces.entry(tx.app).or_default();
                app_nonce.verify_tx(tx, &self.domain)
            })
            .collect())
    }
}

pub struct AppState {
    pub domain: Eip712Domain,
    pub address: Address,
    pub nonces: AppNonces,
}

impl AppState {
    pub fn verify_batch(&mut self, raw_batch: &[u8]) -> postcard::Result<Vec<Transaction>> {
        let batch: Batch = postcard::from_bytes(raw_batch)?;

        Ok(batch
            .txs
            .iter()
            .filter_map(|tx| {
                if self.address != tx.app {
                    return None;
                }

                self.nonces.verify_tx(tx, &self.domain)
            })
            .collect())
    }
}

pub struct AppNonces {
    // user address to nonce
    pub nonces: HashMap<Address, u64>,
}

impl AppNonces {
    pub fn verify_tx(
        &mut self,
        tx: &WireTransaction,
        domain: &Eip712Domain,
    ) -> Option<Transaction> {
        let Some(tx) = tx.verify(&domain) else {
            return None;
        };

        let expected_nonce = self.nonces.entry(tx.sender).or_insert(0);

        if *expected_nonce != tx.nonce {
            return None;
        }

        *expected_nonce += 1;
        Some(tx)
    }
}

impl Default for AppNonces {
    fn default() -> Self {
        Self {
            nonces: HashMap::new(),
        }
    }
}

pub struct Transaction {
    pub sender: Address,
    pub app: Address,
    pub nonce: u64,
    pub max_gas_price: u64,

    pub data: Vec<u8>,
}

sol! {
   #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct SigningMessage {
        address app;
        uint64 nonce;
        uint64 max_gas_price;
        bytes data;
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireTransaction {
    pub app: Address,
    pub nonce: u64,
    pub max_gas_price: u64,
    pub data: Vec<u8>,
    pub signature: Signature,
}

impl WireTransaction {
    pub fn from_signed_transaction(value: &SignedTransaction) -> Self {
        Self {
            app: value.message.app,
            nonce: value.message.nonce,
            max_gas_price: value.message.max_gas_price,
            data: value.message.data.to_vec(),
            signature: value.signature,
        }
    }

    pub fn to_signed_transaction(&self) -> SignedTransaction {
        SignedTransaction {
            message: SigningMessage {
                app: self.app,
                nonce: self.nonce,
                max_gas_price: self.max_gas_price,
                data: self.data.clone().into(),
            },
            signature: self.signature,
        }
    }

    pub fn verify(&self, domain: &Eip712Domain) -> Option<Transaction> {
        let Ok(sender) = self.to_signed_transaction().recover(domain) else {
            return None;
        };

        Some(Transaction {
            sender,
            app: self.app,
            nonce: self.nonce,
            max_gas_price: self.max_gas_price,
            data: self.data.clone(),
        })
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Batch {
    pub sequencer_payment_address: Address,
    pub txs: Vec<WireTransaction>,
}

impl Batch {
    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(&self).unwrap()
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BatchBuilder {
    pub sequencer_payment_address: Address,
    pub txs: Vec<SignedTransaction>,
}

impl BatchBuilder {
    pub fn new(sequencer_payment_address: Address) -> Self {
        Self {
            sequencer_payment_address,
            txs: Vec::new(),
        }
    }

    pub fn add(&mut self, tx: SignedTransaction) {
        self.txs.push(tx)
    }

    pub fn build(self) -> Batch {
        let txs = self
            .txs
            .iter()
            .map(WireTransaction::from_signed_transaction)
            .collect();

        Batch {
            sequencer_payment_address: self.sequencer_payment_address,
            txs,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedTransaction {
    pub message: SigningMessage,
    pub signature: Signature,
}

impl SignedTransaction {
    pub fn valdiate(&self, domain: &Eip712Domain) -> bool {
        self.recover(domain).is_ok()
    }

    pub fn recover(&self, domain: &Eip712Domain) -> Result<Address, SignatureError> {
        let signing_hash = self.message.eip712_signing_hash(&domain);
        self.signature.recover_address_from_prehash(&signing_hash)
    }
}

pub const DOMAIN: Eip712Domain = eip712_domain!(
   name: "CartesiPaio",
   version: "0.0.1",
   chain_id: 1337,
   verifying_contract: Address::ZERO,
);

#[cfg(test)]
mod tests {
    use alloy_core::sol_types::SolStruct;
    use alloy_signer::SignerSync;
    use alloy_signer_wallet::LocalWallet;
    use std::str::FromStr;

    use super::*;

    fn produce_tx() -> (String, Address) {
        let json = r#"
        {
            "app":"0x0000000000000000000000000000000000000000",
            "nonce":0,
            "max_gas_price":0,
            "data":"0x48656c6c6f2c20576f726c6421"
        }
        "#;

        let v: SigningMessage = serde_json::from_str(json).unwrap();
        let signer = LocalWallet::from_str(
            "8114fae7aa0a92c7e3a6015413a54539b4ba9f28254a70f67a3969d73c33509b",
        )
        .unwrap();
        assert_eq!(
            alloy_core::hex::encode(signer.to_field_bytes()),
            "8114fae7aa0a92c7e3a6015413a54539b4ba9f28254a70f67a3969d73c33509b"
        );
        assert_eq!(
            "0x7306897365c277A6951FDA9519fD0CCc16341E4A",
            signer.address().to_string()
        );

        let signature = signer.sign_typed_data_sync(&v, &DOMAIN).unwrap();
        assert_eq!(
            r#"{"r":"0xfa6f7fd6825c953b355c8970fd2c9322162987bfb6898aa78f74f2be6bf8b10c","s":"0x9a2018a7e31b623a91802147e6f8d5c658e17191e69f6663052efda71db72e2","yParity":"0x1"}"#,
            serde_json::to_string(&signature).unwrap()
        );
        let signed_tx = SignedTransaction {
            message: v,
            signature,
        };

        let ret = serde_json::to_string(&signed_tx).unwrap();

        assert_eq!(
            r#"{"message":{"app":"0x0000000000000000000000000000000000000000","nonce":0,"max_gas_price":0,"data":"0x48656c6c6f2c20576f726c6421"},"signature":{"r":"0xfa6f7fd6825c953b355c8970fd2c9322162987bfb6898aa78f74f2be6bf8b10c","s":"0x9a2018a7e31b623a91802147e6f8d5c658e17191e69f6663052efda71db72e2","yParity":"0x1"}}"#,
            ret
        );

        (ret, signer.address())
    }

    #[test]
    fn test() {
        let (tx_json, signer) = produce_tx(); // metamask
        println!("JSON: {tx_json}");

        let tx: SignedTransaction = serde_json::from_str(&tx_json).unwrap();
        let signing_hash = tx.message.eip712_signing_hash(&DOMAIN);
        let recovered = tx
            .signature
            .recover_address_from_prehash(&signing_hash)
            .unwrap();

        assert_eq!(signer, recovered);

        assert_eq!(
            r#"{"name":"CartesiPaio","version":"0.0.1","chainId":"0x539","verifyingContract":"0x0000000000000000000000000000000000000000"}"#,
            serde_json::to_string(&DOMAIN).unwrap()
        );
    }
}
