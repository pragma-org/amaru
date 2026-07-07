{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DeriveGeneric #-}

module Data.StakeDistribution.Account
    ( AccountSummary (..)
    , AccountsSummary (..)
    , mkAccountSummary
    , mkAccountsSummary
    ) where

import Relude

import Cardano.Ledger.Coin
    ( Coin
    , CompactForm
    )
import Cardano.Ledger.Compactible
    ( fromCompact
    )
import Cardano.Ledger.Credential
    ( Credential
        ( KeyHashObj
        , ScriptHashObj
        )
    )
import Cardano.Ledger.DRep
    ( DRep
    )
import Cardano.Ledger.Keys
    ( KeyRole (..)
    )
import Cardano.Ledger.State
    ( AccountState
    , EraAccounts
        ( balanceAccountStateL
        , stakePoolDelegationAccountStateL
        )
    )
import Data.Aeson
    ( KeyValue ((.=))
    , ToJSON (toEncoding, toJSON)
    , Value (Object)
    , pairs
    )
import Data.Coin
    ( JsonCoin (JsonCoin)
    )
import Data.KeyHash
    ( JsonKeyHash (JsonKeyHash)
    )
import Data.PoolId
    ( JsonPoolId (JsonPoolId)
    )
import Data.ScriptHash
    ( JsonScriptHash (JsonScriptHash)
    )
import Data.StakeDistribution.DRep
    ( DRepReference
    , toDRepReference
    )
import Lens.Micro
    ( (^.)
    )
import Ouroboros.Consensus.Cardano.Block
    ( ConwayEra
    )

import qualified Data.Map.Strict as Map

data AccountSummary = AccountSummary
    { balance :: !JsonCoin
    , drep :: !(Maybe DRepReference)
    , pool :: !(Maybe JsonPoolId)
    }
    deriving (Generic)

instance ToJSON AccountSummary where
    toJSON =
        Object . accountSummaryFields

    toEncoding =
        pairs . accountSummaryFields

data AccountsSummary = AccountsSummary
    { verificationKeys :: !(Map JsonKeyHash AccountSummary)
    , scripts :: !(Map JsonScriptHash AccountSummary)
    }
    deriving (Generic)

instance ToJSON AccountsSummary where
    toJSON =
        Object . accountsSummaryFields

    toEncoding =
        pairs . accountsSummaryFields

accountSummaryFields :: (KeyValue e kv, Monoid kv) => AccountSummary -> kv
accountSummaryFields AccountSummary{balance, pool, drep} = mempty
    <> "balance" .= balance
    <> "drep" .= drep
    <> "pool" .= pool

accountsSummaryFields :: (KeyValue e kv, Monoid kv) => AccountsSummary -> kv
accountsSummaryFields AccountsSummary{verificationKeys, scripts} = mempty
    <> "verification_keys" .= verificationKeys
    <> "scripts" .= scripts

mkAccountSummary
    :: Map.Map (Credential Staking) (CompactForm Coin)
    -> Map.Map (Credential Staking) DRep
    -> Credential Staking
    -> AccountState ConwayEra
    -> AccountSummary
mkAccountSummary instantStake dRepDelegatees credential accountState =
    AccountSummary
        { balance = JsonCoin (rewardBalance <> stakeBalance)
        , pool = JsonPoolId <$> (accountState ^. stakePoolDelegationAccountStateL)
        , drep = toDRepReference <$> Map.lookup credential dRepDelegatees
        }
  where
    rewardBalance =
        fromCompact (accountState ^. balanceAccountStateL)

    stakeBalance =
        maybe mempty fromCompact (Map.lookup credential instantStake)

mkAccountsSummary :: Map.Map (Credential Staking) AccountSummary -> AccountsSummary
mkAccountsSummary accountSummaries =
    AccountsSummary
        { verificationKeys
        , scripts
        }
  where
    (verificationKeys, scripts) =
        Map.foldlWithKey' partitionAccounts (mempty, mempty) accountSummaries

    partitionAccounts (verificationKeysMap, scriptsMap) credential accountSummary =
        case credential of
            KeyHashObj keyHash ->
                ( Map.insert (JsonKeyHash keyHash) accountSummary verificationKeysMap
                , scriptsMap
                )
            ScriptHashObj scriptHash ->
                ( verificationKeysMap
                , Map.insert (JsonScriptHash scriptHash) accountSummary scriptsMap
                )
