module Data.RewardAccount
    ( JsonRewardAccount (..)
    ) where

import Relude

import Cardano.Ledger.Address
    ( RewardAccount
    , serialiseRewardAccount
    )
import Data.Aeson
    ( ToJSON (toJSON)
    )
import Data.HexString
    ( JsonHexString (JsonHexString)
    )

newtype JsonRewardAccount = JsonRewardAccount
    { unJsonRewardAccount :: RewardAccount
    }

instance ToJSON JsonRewardAccount where
    toJSON =
        toJSON . JsonHexString . serialiseRewardAccount . unJsonRewardAccount
