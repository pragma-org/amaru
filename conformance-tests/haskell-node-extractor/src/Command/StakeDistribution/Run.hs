module Command.StakeDistribution.Run
    ( run
    ) where

import Relude

import Command.StakeDistribution.Error
    ( Error (SnapshotError)
    )
import Command.StakeDistribution.Parse
    ( Options (..)
    )
import Data.Aeson.Encode.Pretty
    ( encodePretty'
    )
import Command.ExtractSnapshot.Run
    ( LoadedSnapshot (..)
    , loadSnapshot
    )
import Data.NetworkName
    ( networkNameToNetwork
    )
import Query.StakeDistribution
    ( queryStakeDistribution
    )
import Data.StakeDistribution
    ( jsonConfig
    )

import qualified Data.ByteString.Lazy as LBS

run :: Options -> ExceptT Error IO ()
run Options{networkName, snapshotPath} = do
    loadedSnapshot <- ExceptT (first SnapshotError <$> runExceptT (loadSnapshot snapshotPath))
    let epochNumber = loadedSnapshotEpochNumber loadedSnapshot

    let stakeDistribution =
            queryStakeDistribution
                (networkNameToNetwork networkName)
                epochNumber
                (loadedSnapshotState loadedSnapshot)

    liftIO (LBS.putStr (encodePretty' jsonConfig stakeDistribution))
