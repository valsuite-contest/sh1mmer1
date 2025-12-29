# M1NSH1M (Rust)

A Rust rewrite of the M1NSH1M/wax toolchain for ChromeOS shim modification.

## Overview

M1NSH1M (originally SH1MMER) is a shim modifying automation tool that allows
creating modified factory shim images for ChromeOS devices. This Rust
implementation provides the same functionality with better performance and
reliability.

## Features

- **wax** - Main shim modification tool
- **strip** - Remove payloads from shim images
- **gpt2image** - Convert block devices to GPT image files
- **gpt-truncate** - Truncate GPT images to minimal size
- **kern-archive** - Archive kernel partitions
- **kern-patch-check** - Check kernel patch status
- **kern-sig-check** - Check kernel signatures
- **kern-sig-archive** - Archive kernel signatures from firmware
- **kern-sig-shortgen** - Generate short kernel signature list
- **update-downloader** - Download ChromeOS update payloads
- **misc-check** - Check miscellaneous info about shim images
- **rma-merge** - Merge multiple RMA shim images
- **wax4web-bundler** - Bundle wax4web for distribution
- **gpt** - GPT partition table operations

## Building

```bash
cd wax_rust
cargo build --release
```

The binary will be available at `target/release/m1nsh1m`.

## Usage

```bash
# Show help
m1nsh1m --help

# Modify a shim image with M1NSH1M payloads
sudo m1nsh1m wax -i /path/to/shim.bin

# Strip payloads from a shim image
sudo m1nsh1m strip -i /path/to/shim.bin

# Convert a block device to an image file
sudo m1nsh1m gpt2image /dev/sdX output.bin

# Truncate GPT images
m1nsh1m gpt-truncate image1.bin image2.bin

# Check kernel patch status
sudo m1nsh1m kern-patch-check /path/to/shim.bin
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

## Credits

- Mercury Workshop team
- CoolElectronics, Sharp_Jack, r58playz, Rafflesia, OlyB

## License

MIT License
