{-# LANGUAGE DuplicateRecordFields #-}

module Data.PoolRelay
    ( JsonPoolRelay (..)
    ) where

import Relude

import Cardano.Ledger.BaseTypes
    ( DnsName (dnsToText)
    , Port (portToWord16)
    )
import Cardano.Ledger.PoolParams
    ( StakePoolRelay
        ( MultiHostName
        , SingleHostAddr
        , SingleHostName
        )
    )
import Data.Aeson
    ( ToJSON (toJSON)
    , object
    , (.=)
    )
import Data.Aeson.Key
    ( fromText
    )
import Data.Aeson.Types
    ( Pair
    )
import Data.Maybe.Strict
    ( StrictMaybe
        ( SJust
        , SNothing
        )
    )

newtype JsonPoolRelay = JsonPoolRelay
    { unJsonPoolRelay :: StakePoolRelay
    }

instance ToJSON JsonPoolRelay where
    toJSON (JsonPoolRelay relay) =
        case relay of
            SingleHostAddr port ipv4 ipv6 ->
                object $
                    [ "type" .= ("ipAddress" :: Text)
                    ]
                        <> maybePair "ipv4" ((show <$> strictMaybeToMaybe ipv4) :: Maybe Text)
                        <> maybePair "ipv6" ((show <$> strictMaybeToMaybe ipv6) :: Maybe Text)
                        <> maybePair "port" (portToWord16 <$> strictMaybeToMaybe port)
            SingleHostName port dns ->
                object $
                    [ "type" .= ("hostname" :: Text)
                    , "hostname" .= dnsToText dns
                    ]
                        <> maybePair "port" (portToWord16 <$> strictMaybeToMaybe port)
            MultiHostName dns ->
                object
                    [ "type" .= ("hostname" :: Text)
                    , "hostname" .= dnsToText dns
                    ]

strictMaybeToMaybe :: StrictMaybe a -> Maybe a
strictMaybeToMaybe = \case
    SNothing ->
        Nothing
    SJust value ->
        Just value

maybePair :: ToJSON a => Text -> Maybe a -> [Pair]
maybePair fieldName = \case
    Nothing ->
        []
    Just fieldValue ->
        [ fromText fieldName .= fieldValue
        ]
