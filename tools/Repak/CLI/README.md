# repak-rivals

Library and CLI tool for working with Unreal Engine .pak files. Modified to work with netease pak files and patch uasset meshes for marvel rivals

 - Supports reading and writing a wide range of versions
 - Easy to use API while providing low level control:
   - Only parses index initially and reads file data upon request
   - Can rewrite index in place to perform append or delete operations without rewriting entire pak

## Mod patching instructions
1) First, grab repak for your platform under Download repak_cli 0.X.X and extract it somewhere for easy access. Currently, Windows and Linux are supported.
2) Move the mod .pak file inside the repak folder, now drag the mod .pak into repakMod.bat and you'll get a .pak file that's now fixed. Take it and drop it in your Paks folder. (or ~mods folder inside your Paks folder for the sake of organization)
Make sure that _9999999 precedes the _P suffix. So "zDopeAssMod_P" would become "zDopeAssMod_9999999_P".

3) This is required in order for the game to prioritize mod files and replace the assets properly.

The fix for custom models is applied on repacking, so no further tweaking is needed. Enjoy!

As for modders:
`repakMod.bat` can also be used to pack folders into `.pak` files! There's also a `.bat` script for unpacking `.pak` files into folders too if you need it.


*If you like my support on this game consider [buying me a coffee](https://ko-fi.com/natimerry)*


## acknowledgements
- [unpak](https://github.com/bananaturtlesandwich/unpak): original crate featuring read-only pak operations
- [rust-u4pak](https://github.com/panzi/rust-u4pak)'s README detailing the pak file layout
- [jieyouxu](https://github.com/jieyouxu) for serialization implementation of the significantly more complex V11 index
- [repak](https://github.com/trumank/repak) for the original repak implementation
