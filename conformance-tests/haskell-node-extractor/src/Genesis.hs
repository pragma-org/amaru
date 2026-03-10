module Genesis
    ( Genesis (..)
    , networkToGenesis
    ) where

import Relude

import Cardano.Ledger.BaseTypes
    ( ActiveSlotCoeff
    , BoundedRational (boundRational)
    , mkActiveSlotCoeff
    )
import Cardano.Ledger.Coin
    ( Coin (Coin)
    )
import Cardano.Ledger.Slot
    ( EpochSize (EpochSize)
    )
import Data.NetworkName
    ( NetworkName
        ( Mainnet
        , Preprod
        , Preview
        )
    )
import Data.Ratio
    ( (%)
    )

data Genesis = Genesis
    { epochSize :: !EpochSize
    , maxSupply :: !Coin
    , activeSlotCoeff :: !ActiveSlotCoeff
    }

networkToGenesis :: NetworkName -> Genesis
networkToGenesis = \case
    Mainnet ->
        -- TODO: Confirm these values against the intended mainnet genesis file.
        Genesis
            { epochSize = EpochSize 432000
            , maxSupply = Coin 45000000000000000
            , activeSlotCoeff = knownActiveSlotCoeff (1 % 20)
            }
    Preprod ->
        -- TODO: Replace these placeholders with the preprod genesis values.
        Genesis
            { epochSize = EpochSize 432000
            , maxSupply = Coin 45000000000000000
            , activeSlotCoeff = knownActiveSlotCoeff (1 % 20)
            }
    Preview ->
        -- TODO: Replace these placeholders with the preview genesis values.
        Genesis
            { epochSize = EpochSize 86400
            , maxSupply = Coin 45000000000000000
            , activeSlotCoeff = knownActiveSlotCoeff (1 % 20)
            }

knownActiveSlotCoeff :: Rational -> ActiveSlotCoeff
knownActiveSlotCoeff rationalValue =
    mkActiveSlotCoeff positiveUnitInterval
  where
    positiveUnitInterval =
        case boundRational rationalValue of
            Just value ->
                value
            Nothing ->
                error "Invalid ActiveSlotCoeff placeholder in Genesis.networkToGenesis"
