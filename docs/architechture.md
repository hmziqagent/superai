I want this project to be rust.
And I want this to be super modular.


Layers:
1) Configs:
    layer that would parse encode/decode the configs while how every they're set by different provider.
    Bracket could be JSON, TOML or whatever.
2) Core:
    Core would be responsible for all the adding, updating, removing, mutating , backups and whole shebang.
3) Interface:
    We are targeting GPUI first so we will be using GPUI
