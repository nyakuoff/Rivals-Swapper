# Unreleased

Nothing Yet!

# Version 2.5.4 (2025-05-06)

## Changes:
- Fix removal of tempdir causes issues in install mods


# Version 2.5.2 (2025-05-06)

This release contains a window asking users to donate to the project.

## Changes:
- Clean up temporary directories after creating them.

# Version 2.5.1 (2025-05-06)

## New features:
- Ability to install mods from zip files directly
- Show packaging type in install window
- Allow users to unselect specific mods when installing in batch

# Version 2.4.0 (2025-05-05)

This release contains code simplification and bug fixes.

## Changes:
- Added ability to fix dragged .zip/.rar files containing one or more `.pak` files into repak gui 

# Version 2.4.0 (2025-05-04)

This release contains QOL improvements and movie mod fixes for the mod manager.

## Changes:
- Simplify mod type detection
- Add mod type detection for IOStore mods
- Add emma frost skin names to mod categories

## What's broken
- Modtype while importing zip / rar files still doesnt work. This requires extra work

# Version 2.3.0 (2025-05-04)

This release contains QOL improvements and movie mod fixes for the mod manager.

Changes:
 - Removed option to set audio mod manually, this is done automatically for audio mods now.
 - Make movie mods pak the same way as audio mods
 - Remove retrieving filenames from chunkname, instead use the iostore manifest