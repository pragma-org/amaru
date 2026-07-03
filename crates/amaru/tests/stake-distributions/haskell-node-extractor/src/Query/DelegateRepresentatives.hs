{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE NamedFieldPuns #-}

module Query.DelegateRepresentatives
    ( delegateRepresentativesOutputPath
    , queryDelegateRepresentatives
    ) where

import Relude

import Cardano.Ledger.Api.State.Query
    ( queryDRepDelegations
    , queryDRepStakeDistr
    , queryDRepState
    )
import Cardano.Ledger.Coin
    ( Coin
    )
import Cardano.Ledger.Compactible
    ( fromCompact
    )
import Cardano.Ledger.Credential
    ( Credential
    )
import Cardano.Ledger.Keys
    ( KeyRole (..)
    )
import Cardano.Ledger.Shelley.LedgerState
    ( NewEpochState
    )
import Data.DelegateRepresentative
    ( DRep (..)
    , PredefinedDRep (..)
    , RegisteredDRep (..)
    )
import Data.Mandate
    ( Mandate (Mandate)
    )
import Ouroboros.Consensus.Cardano.Block
    ( ConwayEra
    )

import qualified Data.Map.Merge.Strict as Merge
import qualified Data.Map.Strict as Map
import qualified Data.Set as Set
import qualified Cardano.Ledger.DRep as Ledger

delegateRepresentativesOutputPath :: Word64 -> FilePath
delegateRepresentativesOutputPath epochNumber =
    "dreps/" <> toString (show epochNumber :: Text) <> ".json"

queryDelegateRepresentatives :: NewEpochState ConwayEra -> [DRep]
queryDelegateRepresentatives newEpochState =
    Map.elems $
        mergeDelegations
            ( mergeStateAndStake
                registeredDRepStates
                dRepStakeDistribution
            )
            dRepDelegations
  where
    registeredDRepStates =
        Map.fromList
            [ (Ledger.credToDRep credential, (credential, st))
            | (credential, st) <- Map.toList (queryDRepState newEpochState Set.empty)
            ]
    dRepStakeDistribution =
        queryDRepStakeDistr newEpochState Set.empty
    dRepDelegations =
        queryDRepDelegations newEpochState Set.empty

mergeStateAndStake
    :: Map.Map Ledger.DRep (Credential DRepRole, Ledger.DRepState)
    -> Map.Map Ledger.DRep Coin
    -> Map.Map Ledger.DRep DRep
mergeStateAndStake dRepStates dRepStakes =
    Merge.merge
        (Merge.mapMissing $ \_ (credential, dRepState) -> registeredDRep credential dRepState mempty)
        (Merge.mapMissing $ \drep -> predefinedDRep drep)
        (Merge.zipWithMatched $ \_ (credential, dRepState) -> registeredDRep credential dRepState)
        dRepStates
        dRepStakes

mergeDelegations
    :: Map.Map Ledger.DRep DRep
    -> Map.Map Ledger.DRep (Set.Set (Credential Staking))
    -> Map.Map Ledger.DRep DRep
mergeDelegations dReps dRepDelegations =
    Merge.merge
        (Merge.mapMissing $ \_ -> identity)
        (Merge.mapMaybeMissing $ \drep delegators ->
            case drep of
                Ledger.DRepAlwaysAbstain      -> Just (setDelegators delegators (predefinedDRep drep mempty))
                Ledger.DRepAlwaysNoConfidence -> Just (setDelegators delegators (predefinedDRep drep mempty))
                _                        -> error ("DRep has delegation but not stake or state: " <> show drep))
        (Merge.zipWithMatched $ \_ delegateRepresentative delegators -> setDelegators delegators delegateRepresentative)
        dReps
        dRepDelegations

predefinedDRep
    :: Ledger.DRep
    -> Coin
    -> DRep
predefinedDRep dRep stake = case dRep of
    Ledger.DRepAlwaysAbstain ->
        AbstainDelegateRepresentative predefined
    Ledger.DRepAlwaysNoConfidence ->
        NoConfidenceDelegateRepresentative predefined
    _ ->
        error ("Registered DRep unexpectedly missing from queryDRepState results: " <> show dRep)
  where
    predefined = PredefinedDRep{stake, delegators = Set.empty}

registeredDRep
    :: Credential DRepRole
    -> Ledger.DRepState
    -> Coin
    -> DRep
registeredDRep credential dRepState stake =
    RegisteredDelegateRepresentative
        RegisteredDRep
            { credential
            , mandate = Mandate (Ledger.drepExpiry dRepState)
            , deposit = fromCompact (Ledger.drepDeposit dRepState)
            , stake
            , delegators = Set.empty
            }

setDelegators
    :: Set.Set (Credential Staking)
    -> DRep
    -> DRep
setDelegators delegators = \case
    RegisteredDelegateRepresentative RegisteredDRep{credential, deposit, mandate, stake} ->
        RegisteredDelegateRepresentative RegisteredDRep{credential, mandate, deposit, stake, delegators}
    AbstainDelegateRepresentative PredefinedDRep{stake} ->
        AbstainDelegateRepresentative PredefinedDRep{stake, delegators}
    NoConfidenceDelegateRepresentative PredefinedDRep{stake} ->
        NoConfidenceDelegateRepresentative PredefinedDRep{stake, delegators}
