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
    , help
    , long
    , metavar
    , strOption
    )

data Options = Options
    { networkName :: !NetworkName
    , snapshotPath :: !FilePath
    }

optionsParser :: Parser Options
optionsParser =
    Options
        <$> networkNameParser
        <*> strOption
            ( long "snapshot"
                <> metavar "PATH"
                <> help "Path to the consensus ledger snapshot directory (the epoch is derived from the snapshot)"
            )
