{-# LANGUAGE DataKinds #-}
{-# LANGUAGE PatternSynonyms #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE TypeApplications #-}

module Data.Fixture.Common
    ( boundedIntegral
    , boundedRatio
    , compactCoin
    , compactCoinOrError
    , parseCborHex
    , parsePoolId
    , parseScriptHash
    , parseVrfKeyHash
    , perasDisabled
    , renderRatio
    , ratioFromJson
    , showText
    ) where

import Relude

import Cardano.Ledger.BaseTypes
    ( BoundedRational (boundRational)
    )
import Cardano.Ledger.Binary
    ( DecCBOR
    , decodeFull'
    )
import Cardano.Ledger.Binary.Plain
    ( withHexText
    )
import Cardano.Ledger.Coin
    ( Coin
    )
import Cardano.Ledger.Compactible
    ( CompactForm
    , Compactible (toCompact)
    )
import Cardano.Ledger.Conway
    ( ConwayEra
    )
import Cardano.Ledger.Core
    ( eraProtVerLow
    )
import Cardano.Ledger.Hashes
    ( KeyHash
    , KeyRoleVRF (StakePoolVRF)
    , ScriptHash
    , VRFVerKeyHash
    )
import Cardano.Ledger.Keys
    ( KeyRole (StakePool)
    )
import Data.Aeson
    ( fromJSON
    , Result (Error, Success)
    , Value (String)
    , withObject
    , withText
    , (.:)
    )
import Data.Aeson.Types
    ( Parser
    )
import Data.Char
    ( isHexDigit
    )
import Data.Ratio
    ( (%)
    )
import qualified Data.Text as Text
import Ouroboros.Consensus.HardFork.History.EraParams
    ( PerasEnabled
    , pattern NoPerasEnabled
    )

showText :: Show a => a -> Text
showText value =
    toText (show value :: String)

boundedIntegral :: forall a b. (Integral a, Integral b, Bounded b, Show a) => Text -> a -> Either Text b
boundedIntegral contextLabel value =
    if numericValue < lowerBound || numericValue > upperBound
        then Left (contextLabel <> " is out of bounds: " <> showText value)
        else Right (fromIntegral value)
  where
    numericValue = toInteger value
    lowerBound = toInteger (minBound @b)
    upperBound = toInteger (maxBound @b)

boundedRatio :: forall r. BoundedRational r => Text -> Rational -> Either Text r
boundedRatio contextLabel ratioValue =
    maybe
        (Left (contextLabel <> " is outside the supported bounds: " <> renderRatio ratioValue))
        Right
        (boundRational ratioValue)

ratioFromJson :: Value -> Parser Rational
ratioFromJson =
    withObject "Ratio" $ \objectValue -> do
        numer <- objectValue .: "numerator"
        denom <- objectValue .: "denominator"
        if denom <= (0 :: Integer)
            then fail "ratio denominator must be strictly positive"
            else pure (numer % denom)

renderRatio :: Rational -> Text
renderRatio rationalValue =
    show (numerator rationalValue) <> "/" <> show (denominator rationalValue)

parseCborHex :: forall a. DecCBOR a => Text -> Text -> Parser a
parseCborHex contextLabel hexText =
    case withHexText (decodeFull' (eraProtVerLow @ConwayEra)) hexText of
        Right value ->
            pure value
        Left err ->
            fail (toString (contextLabel <> " failed to decode from CBOR hex: " <> showText err))

parsePoolId :: Value -> Parser (KeyHash StakePool)
parsePoolId =
    withText "PoolId" $ \hexText ->
        if Text.length hexText == 56 && Text.all isHexDigit hexText
            then case fromJSON (String hexText) of
                Success poolId ->
                    pure poolId
                Error err ->
                    fail ("Invalid pool id hex: " <> err)
            else fail ("Invalid pool id hex: " <> toString hexText)

parseScriptHash :: Value -> Parser ScriptHash
parseScriptHash =
    withText "ScriptHash" $ \hexText ->
        if Text.length hexText == 56 && Text.all isHexDigit hexText
            then case fromJSON (String hexText) of
                Success scriptHash ->
                    pure scriptHash
                Error err ->
                    fail ("Invalid script hash hex: " <> err)
            else fail ("Invalid script hash hex: " <> toString hexText)

parseVrfKeyHash :: Value -> Parser (VRFVerKeyHash StakePoolVRF)
parseVrfKeyHash =
    withText "VrfKeyHash" $ \hexText ->
        if Text.length hexText == 64 && Text.all isHexDigit hexText
            then case fromJSON (String hexText) of
                Success vrfKeyHash ->
                    pure vrfKeyHash
                Error err ->
                    fail ("Invalid VRF key hash hex: " <> err)
            else fail ("Invalid VRF key hash hex: " <> toString hexText)

compactCoin :: Text -> Coin -> Either Text (CompactForm Coin)
compactCoin contextLabel coinValue =
    maybe
        (Left ("cannot compact " <> contextLabel <> ": " <> showText coinValue))
        Right
        (toCompact coinValue)

compactCoinOrError :: Text -> Coin -> CompactForm Coin
compactCoinOrError contextLabel coinValue =
    case compactCoin contextLabel coinValue of
        Right compactValue ->
            compactValue
        Left _ ->
            error ("Invalid compact coin for " <> contextLabel)

perasDisabled :: PerasEnabled a
perasDisabled =
    NoPerasEnabled
