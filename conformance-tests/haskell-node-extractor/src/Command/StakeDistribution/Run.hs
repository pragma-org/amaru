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
run Options{networkName, outputDir, snapshotPath} = do
    loadedSnapshot <- ExceptT (first SnapshotError <$> runExceptT (loadSnapshot snapshotPath))

    let epochNumber = loadedSnapshotEpochNumber loadedSnapshot

    putStrLn $ "Loaded valid snapshot at epoch=" <> show epochNumber

    let outputPath =
            outputDir
                </> "stake-distributions"
                </> toString (networkNameToText networkName)
                </> (show epochNumber <.> "json")

    let stakeDistribution =
            queryStakeDistribution
                (networkNameToNetwork networkName)
                epochNumber
                (loadedSnapshotState loadedSnapshot)

    liftIO (writeJsonOutput outputPath jsonConfig stakeDistribution)

    putStrLn $ "Stake distribution extracted to: " <> outputPath
