# NSIS template

`installer.nsi` is derived from Tauri CLI **2.12.0**'s upstream NSIS template
(`crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi`, MIT/Apache-2.0).

The only intentional change is the `currentUser` default install directory:

```nsis
StrCpy $INSTDIR "$LOCALAPPDATA\Programs\${PRODUCTNAME}"
```

This keeps application files separate from user data in `%LOCALAPPDATA%\Ageminal`.
Update this file when upgrading the Tauri CLI, and re-apply that one-line change.
