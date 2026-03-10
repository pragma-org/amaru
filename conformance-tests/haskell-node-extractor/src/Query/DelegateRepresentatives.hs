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
import Cardano.Ledger.Credential
    ( Credential
    , StakeCredential
    )
import Cardano.Ledger.DRep
    ( DRep
        ( DRepAlwaysAbstain
        , DRepAlwaysNoConfidence
        )
    , DRepState
        ( drepDeposit
        , drepExpiry
        )
    , credToDRep
    )
import Cardano.Ledger.Keys
    ( KeyRole (DRepRole)
    )
import Cardano.Ledger.Shelley.LedgerState
    ( NewEpochState
    )
import Data.DelegateRepresentative
    ( DelegateRepresentative (..)
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

delegateRepresentativesOutputPath :: Word64 -> FilePath
delegateRepresentativesOutputPath epochNumber =
    "data/dreps/" <> toString (show epochNumber :: Text) <> ".json"

queryDelegateRepresentatives :: NewEpochState ConwayEra -> [DelegateRepresentative]
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
            [ (credToDRep credential, (credential, st))
            | (credential, st) <- Map.toList (queryDRepState newEpochState Set.empty)
            ]
    dRepStakeDistribution =
        queryDRepStakeDistr newEpochState Set.empty
    dRepDelegations =
        queryDRepDelegations newEpochState Set.empty

mergeStateAndStake
    :: Map.Map DRep (Credential 'DRepRole, DRepState)
    -> Map.Map DRep Coin
    -> Map.Map DRep DelegateRepresentative
mergeStateAndStake dRepStates dRepStakes =
    Merge.merge
        (Merge.mapMissing $ \_ (credential, dRepState) -> registeredDRep credential dRepState mempty)
        (Merge.mapMissing $ \drep -> predefinedDRep drep)
        (Merge.zipWithMatched $ \_ (credential, dRepState) -> registeredDRep credential dRepState)
        dRepStates
        dRepStakes

mergeDelegations
    :: Map.Map DRep DelegateRepresentative
    -> Map.Map DRep (Set.Set StakeCredential)
    -> Map.Map DRep DelegateRepresentative
mergeDelegations dReps dRepDelegations =
    Merge.merge
        (Merge.mapMissing $ \_ -> identity)
        (Merge.mapMissing $ \drep _ -> error ("DRep has delegation but not stake or state: " <> show drep))
        (Merge.zipWithMatched $ \_ delegateRepresentative delegators -> setDelegators delegators delegateRepresentative)
        dReps
        dRepDelegations

predefinedDRep
    :: DRep
    -> Coin
    -> DelegateRepresentative
predefinedDRep dRep stake = case dRep of
    DRepAlwaysAbstain ->
        AbstainDelegateRepresentative predefined
    DRepAlwaysNoConfidence ->
        NoConfidenceDelegateRepresentative predefined
    _ ->
        error ("Registered DRep unexpectedly missing from queryDRepState results: " <> show dRep)
  where
    predefined = PredefinedDRep{stake, delegators = Set.empty}

registeredDRep
    :: Credential 'DRepRole
    -> DRepState
    -> Coin
    -> DelegateRepresentative
registeredDRep credential dRepState stake =
    RegisteredDelegateRepresentative
        RegisteredDRep
            { credential
            , mandate = Mandate (drepExpiry dRepState)
            , deposit = drepDeposit dRepState
            , stake
            , delegators = Set.empty
            }

setDelegators
    :: Set.Set StakeCredential
    -> DelegateRepresentative
    -> DelegateRepresentative
setDelegators delegators = \case
    RegisteredDelegateRepresentative RegisteredDRep{credential, deposit, mandate, stake} ->
        RegisteredDelegateRepresentative RegisteredDRep{credential, mandate, deposit, stake, delegators}
    AbstainDelegateRepresentative PredefinedDRep{stake} ->
        AbstainDelegateRepresentative PredefinedDRep{stake, delegators}
    NoConfidenceDelegateRepresentative PredefinedDRep{stake} ->
        NoConfidenceDelegateRepresentative PredefinedDRep{stake, delegators}
