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
    ( Config (..)
    , Indent (..)
    , defConfig
    , encodePretty'
    )
import System.Directory
    ( createDirectoryIfMissing
    )
import System.FilePath
    ( takeDirectory
    )

defaultConfig :: Config
defaultConfig = defConfig
    {  confIndent = Spaces 2
    }

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
    createDirectoryIfMissing True (takeDirectory outputPath)
    writeFileLBS outputPath (encodePretty' config jsonValue)
