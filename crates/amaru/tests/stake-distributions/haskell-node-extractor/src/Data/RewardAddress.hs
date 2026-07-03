module Data.RewardAddress
    ( JsonRewardAddress (..)
    ) where

import Cardano.Ledger.Address
    ( AccountAddress
    , serialiseAccountAddress
    )
import Data.Aeson
    ( ToJSON (toJSON)
    )
import Data.HexString
    ( JsonHexString (JsonHexString)
    )

newtype JsonRewardAddress = JsonRewardAddress
    { unJsonRewardAddress :: AccountAddress
    }

instance ToJSON JsonRewardAddress where
    toJSON (JsonRewardAddress address) =
        toJSON (JsonHexString (serialiseAccountAddress address))
