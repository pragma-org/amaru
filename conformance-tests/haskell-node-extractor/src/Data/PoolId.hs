{-# LANGUAGE DataKinds #-}

module Data.PoolId
    ( JsonPoolId (..)
    , poolIdText
    ) where

import Relude

import Cardano.Ledger.Hashes
    ( KeyHash
    )
import Cardano.Ledger.Keys
    ( KeyRole (StakePool)
    )
import Data.Aeson
    ( ToJSON (toJSON)
    , Value (String)
    )

newtype JsonPoolId = JsonPoolId
    { unJsonPoolId :: KeyHash 'StakePool
    }

instance ToJSON JsonPoolId where
    toJSON =
        String . poolIdText . unJsonPoolId

poolIdText :: KeyHash 'StakePool -> Text
poolIdText poolId =
    case toJSON poolId of
        String textValue ->
            textValue
        _ ->
            error "KeyHash StakePool ToJSON did not produce a JSON string"
