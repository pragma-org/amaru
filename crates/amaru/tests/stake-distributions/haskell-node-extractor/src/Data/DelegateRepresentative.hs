{-# LANGUAGE DataKinds #-}
{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE NamedFieldPuns #-}

module Data.DelegateRepresentative
    ( DRep (..)
    , PredefinedDRep (..)
    , RegisteredDRep (..)
    ) where

import Relude

import Cardano.Ledger.Coin
    ( Coin
    )
import Cardano.Ledger.Credential
    ( Credential
    )
import Cardano.Ledger.Keys
    ( KeyRole (..)
    )
import Data.Aeson
    ( ToJSON (toJSON)
    , object
    , (.=)
    )
import Data.Aeson.Types
    ( Pair
    )
import Data.Coin
    ( JsonCoin (JsonCoin)
    )
import Data.Credential
    ( JsonCredential (JsonCredential)
    , credentialPairs
    )
import Data.Mandate
    ( Mandate
    )

import qualified Data.Set as Set

data DRep
    = RegisteredDelegateRepresentative !RegisteredDRep
    | AbstainDelegateRepresentative !PredefinedDRep
    | NoConfidenceDelegateRepresentative !PredefinedDRep

data RegisteredDRep = RegisteredDRep
    { stake :: !Coin
    , delegators :: !(Set.Set (Credential Staking))
    , credential :: !(Credential DRepRole)
    , mandate :: !Mandate
    , deposit :: !Coin
    }

data PredefinedDRep = PredefinedDRep
    { stake :: !Coin
    , delegators :: !(Set.Set (Credential Staking))
    }

instance ToJSON DRep where
    toJSON = \case
        RegisteredDelegateRepresentative RegisteredDRep{credential, mandate, deposit, stake, delegators} ->
            object $ mconcat
                [ [ "type" .= ("registered" :: Text) ]
                , credentialPairs credential
                , [ "mandate" .= mandate ]
                , [ "deposit" .= JsonCoin deposit ]
                , commonPairs stake delegators
                ]
        AbstainDelegateRepresentative PredefinedDRep{stake, delegators} ->
            object $ mconcat
                [ [ "type" .= ("abstain" :: Text) ]
                , commonPairs stake delegators
                ]
        NoConfidenceDelegateRepresentative PredefinedDRep{stake, delegators} ->
            object $ mconcat
                [ [ "type" .= ("no_confidence" :: Text) ]
                , commonPairs stake delegators
                ]

commonPairs :: Coin -> Set.Set (Credential Staking) -> [Pair]
commonPairs stake delegators =
    [ "stake" .= JsonCoin stake
    , "delegators" .= fmap JsonCredential (Set.toAscList delegators)
    ]
