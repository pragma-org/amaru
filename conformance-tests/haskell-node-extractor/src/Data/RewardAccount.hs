module Data.RewardAccount
    ( JsonRewardAccount (..)
    ) where

import Relude

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

newtype JsonRewardAccount = JsonRewardAccount
    { unJsonRewardAccount :: AccountAddress
    }

instance ToJSON JsonRewardAccount where
    toJSON =
        toJSON . JsonHexString . serialiseAccountAddress . unJsonRewardAccount
