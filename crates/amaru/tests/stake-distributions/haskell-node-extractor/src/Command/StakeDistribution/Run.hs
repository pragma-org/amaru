module Command.StakeDistribution.Run
    ( run
    ) where

import Relude

import Command.StakeDistribution.Error
    ( Error
        ( SnapshotError
        , SnapshotsNotSequential
        )
    )
import Command.StakeDistribution.Parse
    ( Options (..)
    )
import Command.ExtractSnapshot.Run
    ( LoadedSnapshot (..)
    , loadSnapshot
    )
import Data.NetworkName
    ( networkNameToNetwork
    , networkNameToText
    )
import Helpers.Json
    ( writeJsonOutput
    )
import Query.StakeDistribution
    ( queryStakeDistribution
    )
import Data.StakeDistribution
    ( jsonConfig
    )
import System.FilePath
    ( (<.>)
    , (</>)
    )

run :: Options -> ExceptT Error IO ()
run Options{networkName, outputDir, snapshotPath, nextSnapshotPath} = do
    loadedSnapshot <- ExceptT (first SnapshotError <$> runExceptT (loadSnapshot snapshotPath))
    loadedNextSnapshot <- ExceptT (first SnapshotError <$> runExceptT (loadSnapshot nextSnapshotPath))

    let epochNumber = loadedSnapshotEpochNumber loadedSnapshot
    let nextEpochNumber = loadedSnapshotEpochNumber loadedNextSnapshot

    when (nextEpochNumber /= epochNumber + 1) $
        ExceptT (pure (Left (SnapshotsNotSequential epochNumber nextEpochNumber)))

    putStrLn $ "Loaded valid snapshot at epoch=" <> show epochNumber
    putStrLn $ "Loaded next  snapshot at epoch=" <> show nextEpochNumber

    let outputPath =
            outputDir
                </> toString (networkNameToText networkName)
                </> ("epoch_" <> show epochNumber <.> "json")

    let stakeDistribution =
            queryStakeDistribution
                (networkNameToNetwork networkName)
                epochNumber
                (loadedSnapshotState loadedSnapshot)
                (loadedSnapshotState loadedNextSnapshot)

    liftIO (writeJsonOutput outputPath jsonConfig stakeDistribution)

    putStrLn $ "Stake distribution extracted to: " <> outputPath
