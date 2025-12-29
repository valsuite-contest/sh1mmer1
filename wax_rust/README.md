# M1NSH1M (Rust)

A Rust rewrite of the M1NSH1M/wax toolchain for ChromeOS shim modification.

## Overview

M1NSH1M (originally SH1MMER) is a shim modifying automation tool that allows
creating modified factory shim images for ChromeOS devices. This Rust
implementation provides the same functionality with better performance and
reliability.

## Architecture

The codebase follows a modular architecture:

```
src/
├── main.rs           # CLI entry point and command routing
├── core.rs           # Reusable core operations (partition management, etc.)
├── gpt.rs            # GPT partition table handling
├── ext2.rs           # ext2/ext4 filesystem utilities
├── utils.rs          # Common utilities (shell commands, file ops)
└── commands/         # Individual command implementations
    ├── wax.rs        # Full-featured shim modification
    ├── minimal.rs    # Minimal payload boot shim
    ├── stripper.rs   # Payload removal
    └── ...           # Other commands
```

### Core Module (`core.rs`)

The `core` module provides reusable building blocks:

- **LoopDeviceManager** - RAII-based loop device lifecycle management
- **PartitionConfig** - Configuration struct for adding partitions
- **PayloadConfig** - Configuration for copying payloads
- **validate_shim_image()** - Image validation
- **add_partition()** - Add new partitions with automatic resizing
- **copy_payload_to_partition()** - Copy files to partitions
- **shrink_root_partition()** / **squash_partitions()** - Optimize image size

## Features

### Shim Modification Commands

- **wax** - Full-featured shim modification with M1NSH1M payloads
- **minimal** - Create minimal shim for custom payload boot (NEW!)
- **strip** - Remove payloads from shim images

### Utility Commands

- **gpt2image** - Convert block devices to GPT image files
- **gpt-truncate** - Truncate GPT images to minimal size
- **gpt** - GPT partition table operations

### Kernel Analysis Commands

- **kern-archive** - Archive kernel partitions
- **kern-patch-check** - Check kernel patch status
- **kern-sig-check** - Check kernel signatures
- **kern-sig-archive** - Archive kernel signatures from firmware
- **kern-sig-shortgen** - Generate short kernel signature list

### Other Commands

- **update-downloader** - Download ChromeOS update payloads
- **misc-check** - Check miscellaneous info about shim images
- **rma-merge** - Merge multiple RMA shim images
- **wax4web-bundler** - Bundle wax4web for distribution

## Building

```bash
cd wax_rust
cargo build --release
```

The binary will be available at `target/release/m1nsh1m`.

## Usage

### Full Featured Shim (wax)

```bash
# Modify a shim image with M1NSH1M payloads
sudo m1nsh1m wax -i /path/to/shim.bin

# With chromebrew support
sudo m1nsh1m wax -i /path/to/shim.bin --chromebrew chromebrew.tar.gz -s 4G

# Legacy payload
sudo m1nsh1m wax -i /path/to/shim.bin -p legacy
```

### Minimal Custom Payload Shim (NEW!)

The `minimal` command creates a shim with the smallest possible footprint
to boot a custom payload:

```bash
# Create minimal shim with a custom init script
sudo m1nsh1m minimal -i /path/to/shim.bin -x /path/to/my_init.sh

# Create minimal shim with a payload directory
sudo m1nsh1m minimal -i /path/to/shim.bin -p /path/to/payload_dir

# Combine both with custom partition size
sudo m1nsh1m minimal -i /path/to/shim.bin -x init.sh -p payload/ -s 64M

# Fast mode (skip partition shrinking for faster build)
sudo m1nsh1m minimal -i /path/to/shim.bin -x init.sh --fast
```

#### Minimal Command Options

| Option | Description |
|--------|-------------|
| `-i, --image` | Path to factory shim image (required) |
| `-x, --init-script` | Custom init script to execute on boot |
| `-p, --payload-dir` | Directory to copy to the payload partition |
| `-s, --payload-size` | Size for payload partition (default: 16M) |
| `--arch` | Force architecture (x86_64 or aarch64) |
| `--fast` | Skip shrinking partitions (faster build) |

### Other Commands

```bash
# Strip payloads from a shim image
sudo m1nsh1m strip -i /path/to/shim.bin

# Convert a block device to an image file
sudo m1nsh1m gpt2image /dev/sdX output.bin

# Truncate GPT images
m1nsh1m gpt-truncate image1.bin image2.bin

# Check kernel patch status
sudo m1nsh1m kern-patch-check /path/to/shim.bin

# Show help for any command
m1nsh1m <command> --help
```

## Requirements

The following external tools are required for full functionality:

- `sgdisk`, `sfdisk` - GPT manipulation
- `mkfs.ext4`, `mkfs.ext2` - Filesystem creation
- `tune2fs`, `e2fsck`, `resize2fs` - Filesystem management
- `futility` - ChromeOS verified boot utilities
- `cgpt` - ChromeOS partition manipulation
- `tar`, `gzip` - Archive handling
- `binwalk`, `lz4`, `cpio` - Kernel analysis

## Creating Custom Payloads

For the `minimal` command, your init script receives the stateful mount path
as an argument. Example minimal payload:

```bash
#!/bin/sh
# my_payload.sh - Custom payload example
STATEFUL_MNT="$1"

echo "Booted with minimal shim!"
echo "Payload directory: $STATEFUL_MNT"

# Your custom code here...
# Examples:
# - Run diagnostics
# - Install custom software
# - Modify system settings

# Keep running or drop to shell
exec /bin/sh
```

## Credits

- Mercury Workshop team
- CoolElectronics, Sharp_Jack, r58playz, Rafflesia, OlyB

## License

MIT License
