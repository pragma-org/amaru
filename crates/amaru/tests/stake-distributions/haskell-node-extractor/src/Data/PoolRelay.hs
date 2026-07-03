{-# LANGUAGE DuplicateRecordFields #-}

module Data.PoolRelay
    ( JsonPoolRelay (..)
    ) where

import Relude

import Cardano.Ledger.BaseTypes
    ( DnsName (dnsToText)
    , Port (portToWord16)
    )
import Cardano.Ledger.State
    ( StakePoolRelay
        ( MultiHostName
        , SingleHostAddr
        , SingleHostName
        )
    )
import Data.Aeson
    ( KeyValueOmit ((.?=))
    , KeyValue ((.=))
    , ToJSON (toEncoding, toJSON)
    , Value (Object)
    , pairs
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
    toJSON =
        Object . poolRelayFields

    toEncoding =
        pairs . poolRelayFields

poolRelayFields :: (KeyValueOmit e kv, Monoid kv) => JsonPoolRelay -> kv
poolRelayFields (JsonPoolRelay relay) =
        case relay of
            SingleHostAddr port ipv4 ipv6 -> mempty
                <> "type" .= ("ip_address" :: Text)
                <> "ipv4" .?= ((show <$> strictMaybeToMaybe ipv4) :: Maybe Text)
                <> "ipv6" .?= ((show <$> strictMaybeToMaybe ipv6) :: Maybe Text)
                <> "port" .?= (portToWord16 <$> strictMaybeToMaybe port)
            SingleHostName port dns -> mempty
                <> "type" .= ("hostname" :: Text)
                <> "hostname" .= dnsToText dns
                <> "port" .?= (portToWord16 <$> strictMaybeToMaybe port)
            MultiHostName dns -> mempty
                <> "type" .= ("hostname" :: Text)
                <> "hostname" .= dnsToText dns

strictMaybeToMaybe :: StrictMaybe a -> Maybe a
strictMaybeToMaybe = \case
    SNothing ->
        Nothing
    SJust value ->
        Just value
