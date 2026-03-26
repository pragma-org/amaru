module Data.Mandate
    ( Mandate (..)
    ) where

import Cardano.Slotting.Slot
    ( EpochNo (unEpochNo)
    )
import Data.Aeson
    ( ToJSON (toJSON)
    , object
    , (.=)
    )

newtype Mandate = Mandate
    { unMandate :: EpochNo
    }

instance ToJSON Mandate where
    toJSON (Mandate epochNumber) =
        object
            [ "epoch" .= unEpochNo epochNumber
            ]
