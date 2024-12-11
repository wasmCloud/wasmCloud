use testcontainers::{core::WaitFor, Image};

#[derive(Default, Debug, Clone)]
pub struct Azurite {
    _priv: (),
}

impl Image for Azurite {
    fn name(&self) -> &str {
        "mcr.microsoft.com/azure-storage/azurite"
    }

    fn tag(&self) -> &str {
        "3.32.0"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout(
            "Azurite Blob service is successfully listening",
        )]
    }

    // We need to override the command used for running Azurite so that loose mode can be enabled
    fn cmd(&self) -> impl IntoIterator<Item = impl Into<std::borrow::Cow<'_, str>>> {
        vec![
            // Defaults from the Dockerfile:
            // https://github.com/Azure/Azurite/blob/76f626284e4b4b58b95065bb3c92351f30af7f3d/Dockerfile#L47
            "azurite",
            "-l",
            "/data",
            "--blobHost",
            "0.0.0.0",
            "--queueHost",
            "0.0.0.0",
            "--tableHost",
            "0.0.0.0",
            // Loose mode is required for testing the `get-container-data` endpoint, for more info on what the flag does see:
            // https://learn.microsoft.com/en-us/azure/storage/common/storage-use-azurite?tabs=docker-hub%2Cblob-storage#loose-mode
            "--loose",
        ]
    }
}
