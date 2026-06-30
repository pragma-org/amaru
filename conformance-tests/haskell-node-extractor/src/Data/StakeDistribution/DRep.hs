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
    )
import Cardano.Ledger.Keys
    ( KeyRole (..)
    )
import Cardano.Ledger.Slot
    ( EpochNo (unEpochNo)
    )
import Data.Aeson
    ( Options (constructorTagModifier, fieldLabelModifier, omitNothingFields)
    , ToJSON (toJSON)
    , defaultOptions
    , genericToJSON
    )
import Data.Coin
    ( JsonCoin (JsonCoin)
    )
import Data.KeyHash
    ( JsonKeyHash (JsonKeyHash)
    )
import Helpers.Json
    ( snakeCaseFieldLabel
    , snakeCaseOptions
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
        genericToJSON snakeCaseOptions

data DRepSummary = DRepSummary
    { validUntil :: !Word64
    , metadata :: !(Maybe Metadata)
    , votingStake :: !JsonCoin
    }
    deriving (Generic)

instance ToJSON DRepSummary where
    toJSON =
        genericToJSON snakeCaseOptions

newtype DRepStakeSummary = DRepStakeSummary
    { votingStake :: JsonCoin
    }
    deriving (Generic)

instance ToJSON DRepStakeSummary where
    toJSON =
        genericToJSON snakeCaseOptions

data DRepReferenceType
    = Abstain
    | NoConfidence
    | VerificationKey
    | Script
    deriving (Generic)

instance ToJSON DRepReferenceType where
    toJSON =
        genericToJSON
            defaultOptions
                { constructorTagModifier = snakeCaseFieldLabel
                }

data JsonDRepHash
    = JsonDRepKeyHash !JsonKeyHash
    | JsonDRepScriptHash !JsonScriptHash

instance ToJSON JsonDRepHash where
    toJSON = \case
        JsonDRepKeyHash keyHash ->
            toJSON keyHash
        JsonDRepScriptHash scriptHash ->
            toJSON scriptHash

mkDRepsSummary
    :: Map.Map (Credential DRepRole) DRepState
    -> Map.Map (Credential DRepRole) Coin
    -> Map.Map DRep Coin
    -> DRepsSummary
mkDRepsSummary registeredDRepStates registeredDRepStakeDistribution dRepStakeDistribution =
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
                    , votingStake = JsonCoin (Map.findWithDefault mempty credential registeredDRepStakeDistribution)
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
        genericToJSON
            defaultOptions
                { fieldLabelModifier = \case
                    "drepType" ->
                        "type"
                    otherField ->
                        snakeCaseFieldLabel otherField
                , omitNothingFields = True
                }

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
