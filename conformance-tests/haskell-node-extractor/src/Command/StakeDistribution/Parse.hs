module Command.StakeDistribution.Parse
    ( Options (..)
    , optionsParser
    ) where

import Relude

import Command.ExtractSnapshot.Parse
    ( networkNameParser
    )
import Data.NetworkName
    ( NetworkName
    )
import Options.Applicative
    ( Parser
    , bashCompleter
    , completer
    , help
    , long
    , metavar
    , showDefault
    , strOption
    , value
    )

data Options = Options
    { networkName :: !NetworkName
    , snapshotPath :: !FilePath
    , outputDir :: !FilePath
    }

optionsParser :: Parser Options
optionsParser =
    Options
        <$> networkNameParser
        <*> strOption
            ( long "snapshot"
                <> metavar "PATH"
                <> completer (bashCompleter "directory")
                <> help "Path to the consensus ledger snapshot directory (the epoch is derived from the snapshot)"
            )
        <*> strOption
            ( long "output"
                <> metavar "DIR"
                <> completer (bashCompleter "directory")
                <> value "data"
                <> showDefault
                <> help "Base directory where the extracted JSON file will be written"
            )
