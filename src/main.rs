use clap::Parser;
use pipelink_audio_lib::AudioWriterArgs;

pub fn main() -> Result<(), pipewire::Error> {
    let args = AudioWriterArgs::parse();
    pipelink_audio_lib::run_writer(args)
}
