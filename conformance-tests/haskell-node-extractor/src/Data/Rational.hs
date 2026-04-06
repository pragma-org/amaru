module Data.Rational
    ( JsonRational (..)
    ) where

import Relude

import Data.Aeson
    ( ToJSON (toJSON)
    )

newtype JsonRational = JsonRational
    { unJsonRational :: Rational
    }

instance ToJSON JsonRational where
    toJSON (JsonRational rationalValue) =
        toJSON
            [ numerator rationalValue
            , denominator rationalValue
            ]
