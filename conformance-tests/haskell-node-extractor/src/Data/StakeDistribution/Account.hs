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
    ( ToJSON (toJSON)
    , genericToJSON
    )
import Data.Coin
    ( JsonCoin (JsonCoin)
    )
import Data.KeyHash
    ( JsonKeyHash (JsonKeyHash)
    )
import Helpers.Json
    ( snakeCaseOptions
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
    , stakePool :: !(Maybe JsonPoolId)
    , drep :: !(Maybe DRepReference)
    }
    deriving (Generic)

instance ToJSON AccountSummary where
    toJSON =
        genericToJSON snakeCaseOptions

data AccountsSummary = AccountsSummary
    { verificationKey :: !(Map JsonKeyHash AccountSummary)
    , script :: !(Map JsonScriptHash AccountSummary)
    }
    deriving (Generic)

instance ToJSON AccountsSummary where
    toJSON =
        genericToJSON snakeCaseOptions

mkAccountSummary
    :: Map.Map (Credential Staking) (CompactForm Coin)
    -> Map.Map (Credential Staking) DRep
    -> Credential Staking
    -> AccountState ConwayEra
    -> AccountSummary
mkAccountSummary instantStake dRepDelegatees credential accountState =
    AccountSummary
        { balance = JsonCoin (rewardBalance <> stakeBalance)
        , stakePool = JsonPoolId <$> (accountState ^. stakePoolDelegationAccountStateL)
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
        { verificationKey
        , script
        }
  where
    (verificationKey, script) =
        Map.foldlWithKey' partitionAccounts (mempty, mempty) accountSummaries

    partitionAccounts (verificationKeys, scriptHashes) credential accountSummary =
        case credential of
            KeyHashObj keyHash ->
                ( Map.insert (JsonKeyHash keyHash) accountSummary verificationKeys
                , scriptHashes
                )
            ScriptHashObj scriptHash ->
                ( verificationKeys
                , Map.insert (JsonScriptHash scriptHash) accountSummary scriptHashes
                )
