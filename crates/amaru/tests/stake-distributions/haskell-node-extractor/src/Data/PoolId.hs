module Data.PoolId
    ( JsonPoolId (..)
    ) where

import Relude

import Cardano.Ledger.Hashes
    ( KeyHash
    )
import Cardano.Ledger.Keys
    ( KeyRole (..)
    )
import Data.Aeson
    ( ToJSON (..)
    , ToJSONKey (..)
    , Value (String)
    )
import Data.Aeson.Types
    ( toJSONKeyText
    )
import Data.KeyHash
    ( keyHashToText
    )

newtype JsonPoolId = JsonPoolId
    { unJsonPoolId :: KeyHash StakePool
    }
    deriving (Eq, Ord)

instance ToJSON JsonPoolId where
    toJSON =
        String . keyHashToText . unJsonPoolId

instance ToJSONKey JsonPoolId where
    toJSONKey =
        toJSONKeyText (keyHashToText . unJsonPoolId)
