module Helpers.Json
    ( snakeCaseFieldLabel
    , snakeCaseOptions
    , writeJsonOutput
    , defaultConfig
    ) where

import Relude

import Data.Aeson
    ( Options (fieldLabelModifier)
    , ToJSON
    , camelTo2
    , defaultOptions
    )
import Data.Aeson.Encode.Pretty
    ( Config (confCompare)
    , defConfig
    , encodePretty'
    , keyOrder
    )
import System.Directory
    ( createDirectoryIfMissing
    )
import System.FilePath
    ( takeDirectory
    )

defaultConfig :: Config
defaultConfig = defConfig

snakeCaseFieldLabel :: String -> String
snakeCaseFieldLabel =
    camelTo2 '_'

snakeCaseOptions :: Options
snakeCaseOptions =
    defaultOptions
        { fieldLabelModifier = snakeCaseFieldLabel
        }

writeJsonOutput :: ToJSON a => FilePath -> Config -> a -> IO ()
writeJsonOutput outputPath config jsonValue = do
    putTextLn ("...extracting " <> toText outputPath)
    createDirectoryIfMissing True (takeDirectory outputPath)
    writeFileLBS outputPath (encodePretty' config jsonValue)
