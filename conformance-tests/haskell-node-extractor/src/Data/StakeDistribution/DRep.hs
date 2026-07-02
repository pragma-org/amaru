{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE DeriveGeneric #-}

module Data.StakeDistribution.DRep
    ( DRepReference (..)
    , DRepReferenceType (..)
    , DRepStakeSummary (..)
    , DRepSummary (..)
    , DRepsSummary (..)
    , mkDRepsSummary
    , toDRepReference
    ) where

import Relude

import Cardano.Ledger.Coin
    ( Coin
    )
import Cardano.Ledger.Credential
    ( Credential
        ( KeyHashObj
        , ScriptHashObj
        )
    )
import Cardano.Ledger.DRep
    ( DRep
        ( DRepAlwaysAbstain
        , DRepAlwaysNoConfidence
        , DRepKeyHash
        , DRepScriptHash
        )
    , DRepState
        ( drepAnchor
        , drepExpiry
        )
    , credToDRep
    )
import Cardano.Ledger.Keys
    ( KeyRole (..)
    )
import Cardano.Ledger.Slot
    ( EpochNo (unEpochNo)
    )
import Data.Aeson
    ( KeyValue ((.=))
    , KeyValueOmit ((.?=))
    , ToJSON (toEncoding, toJSON)
    , Value (Object, String)
    , pairs
    )
import Data.Coin
    ( JsonCoin (JsonCoin)
    )
import Data.KeyHash
    ( JsonKeyHash (JsonKeyHash)
    )
import Data.Maybe.Strict
    ( strictMaybeToMaybe
    )
import Data.Metadata
    ( Metadata
    , metadataFromAnchor
    )
import Data.ScriptHash
    ( JsonScriptHash (JsonScriptHash)
    )

import qualified Data.Map.Strict as Map

data DRepsSummary = DRepsSummary
    { abstain :: !DRepStakeSummary
    , noConfidence :: !DRepStakeSummary
    , verificationKeys :: !(Map JsonKeyHash DRepSummary)
    , scripts :: !(Map JsonScriptHash DRepSummary)
    }
    deriving (Generic)

instance ToJSON DRepsSummary where
    toJSON =
        Object . dRepsSummaryFields

    toEncoding =
        pairs . dRepsSummaryFields

data DRepSummary = DRepSummary
    { validUntil :: !Word64
    , metadata :: !(Maybe Metadata)
    , votingStake :: !JsonCoin
    }
    deriving (Generic)

instance ToJSON DRepSummary where
    toJSON =
        Object . dRepSummaryFields

    toEncoding =
        pairs . dRepSummaryFields

newtype DRepStakeSummary = DRepStakeSummary
    { votingStake :: JsonCoin
    }
    deriving (Generic)

instance ToJSON DRepStakeSummary where
    toJSON =
        Object . dRepStakeSummaryFields

    toEncoding =
        pairs . dRepStakeSummaryFields

data DRepReferenceType
    = Abstain
    | NoConfidence
    | VerificationKey
    | Script
    deriving (Generic)

instance ToJSON DRepReferenceType where
    toJSON = \case
        Abstain ->
            String "abstain"
        NoConfidence ->
            String "no_confidence"
        VerificationKey ->
            String "verification_key"
        Script ->
            String "script"

    toEncoding = \case
        Abstain ->
            toEncoding ("abstain" :: Text)
        NoConfidence ->
            toEncoding ("no_confidence" :: Text)
        VerificationKey ->
            toEncoding ("verification_key" :: Text)
        Script ->
            toEncoding ("script" :: Text)

data JsonDRepHash
    = JsonDRepKeyHash !JsonKeyHash
    | JsonDRepScriptHash !JsonScriptHash

instance ToJSON JsonDRepHash where
    toJSON = \case
        JsonDRepKeyHash keyHash ->
            toJSON keyHash
        JsonDRepScriptHash scriptHash ->
            toJSON scriptHash

    toEncoding = \case
        JsonDRepKeyHash keyHash ->
            toEncoding keyHash
        JsonDRepScriptHash scriptHash ->
            toEncoding scriptHash

mkDRepsSummary
    :: Map.Map (Credential DRepRole) DRepState
    -> Map.Map DRep Coin
    -> DRepsSummary
mkDRepsSummary registeredDRepStates dRepStakeDistribution =
    DRepsSummary
        { abstain = DRepStakeSummary (JsonCoin (Map.findWithDefault mempty DRepAlwaysAbstain dRepStakeDistribution))
        , noConfidence =
            DRepStakeSummary (JsonCoin (Map.findWithDefault mempty DRepAlwaysNoConfidence dRepStakeDistribution))
        , verificationKeys
        , scripts
        }
  where
    (verificationKeys, scripts) =
        Map.foldlWithKey' partitionDReps (mempty, mempty) registeredDRepStates

    partitionDReps (verificationKeysMap, scriptsMap) credential dRepState =
        let dRepSummary =
                DRepSummary
                    { validUntil = unEpochNo (drepExpiry dRepState)
                    , metadata = strictMaybeToMaybe (drepAnchor dRepState) <&> metadataFromAnchor
                    , votingStake = JsonCoin (Map.findWithDefault mempty (credToDRep credential) dRepStakeDistribution)
                    }
         in case credential of
                KeyHashObj keyHash ->
                    ( Map.insert (JsonKeyHash keyHash) dRepSummary verificationKeysMap
                    , scriptsMap
                    )
                ScriptHashObj scriptHash ->
                    ( verificationKeysMap
                    , Map.insert (JsonScriptHash scriptHash) dRepSummary scriptsMap
                    )

data DRepReference = DRepReference
    { hash :: !(Maybe JsonDRepHash)
    , drepType :: !DRepReferenceType
    }
    deriving (Generic)

instance ToJSON DRepReference where
    toJSON =
        Object . dRepReferenceFields

    toEncoding =
        pairs . dRepReferenceFields

dRepsSummaryFields :: (KeyValue e kv, Monoid kv) => DRepsSummary -> kv
dRepsSummaryFields DRepsSummary{abstain, noConfidence, verificationKeys, scripts} = mempty
    <> "abstain" .= abstain
    <> "no_confidence" .= noConfidence
    <> "verification_keys" .= verificationKeys
    <> "scripts" .= scripts

dRepSummaryFields :: (KeyValue e kv, Monoid kv) => DRepSummary -> kv
dRepSummaryFields DRepSummary{validUntil, metadata, votingStake} = mempty
    <> "valid_until" .= validUntil
    <> "metadata" .= metadata
    <> "voting_stake" .= votingStake

dRepStakeSummaryFields :: (KeyValue e kv) => DRepStakeSummary -> kv
dRepStakeSummaryFields DRepStakeSummary{votingStake} =
    "voting_stake" .= votingStake

dRepReferenceFields :: (KeyValueOmit e kv, Monoid kv) => DRepReference -> kv
dRepReferenceFields DRepReference{hash, drepType} = mempty
    <> "type" .= drepType
    <> "hash" .?= hash

toDRepReference :: DRep -> DRepReference
toDRepReference = \case
    DRepAlwaysAbstain ->
        DRepReference
            { hash = Nothing
            , drepType = Abstain
            }
    DRepAlwaysNoConfidence ->
        DRepReference
            { hash = Nothing
            , drepType = NoConfidence
            }
    DRepKeyHash keyHash ->
        DRepReference
            { hash = Just (JsonDRepKeyHash (JsonKeyHash keyHash))
            , drepType = VerificationKey
            }
    DRepScriptHash scriptHash ->
        DRepReference
            { hash = Just (JsonDRepScriptHash (JsonScriptHash scriptHash))
            , drepType = Script
            }
