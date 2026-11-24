<div align="center">
    <img src="resources/icons/hicolor/scalable/apps/icon.svg" width="250" alt="Mail app icon" />
    <h1>Mail</h1>
    <h3>Your inbox, in focus.</h3>
    <p>A mail client for the COSMIC™ desktop</p>
</div>

![color-schemes-light.png](https://github.com/user-attachments/assets/1db1920a-7ca1-4578-a048-2cada8baf749#gh-light-mode-only)
![color-schemes-dark.png](https://github.com/user-attachments/assets/f944581e-dcd5-4322-833e-20656a0e2709#gh-dark-mode-only)

## Installation

A [justfile](./justfile) is included by default for the [casey/just][just] command runner.

- `just` builds the application with the default `just build-release` recipe
- `just run` builds and runs the application
- `just install` installs the project into the system
- `just vendor` creates a vendored tarball
- `just build-vendored` compiles with vendored dependencies from that tarball
- `just check` runs clippy on the project to check for linter warnings
- `just check-json` can be used by IDEs that support LSP

## Mail accounts

Open **Settings → Mail accounts** to see accounts that are available to link. Select **Link**
and approve Mail access. **Add account** opens the Accounts app, where you can add Google or
custom IMAP/SMTP and enable **Email**. Return to Mail to link the new account.

Mail requests server settings and credentials through `accounts_core`, which owns the broker
client, error classification and mail endpoint validation. Mail only maps those results onto its
local account model. Unlinking an account from Mail keeps it available to your other apps.
Existing accounts configured directly in Mail can still be edited or removed in Mail.

## Background mail

After you open Mail, it checks for new Inbox messages even when its window is
closed. Open Mail again to return to the same session, or choose **Menu → Quit
Mail** to stop background checks. Open Mail once after signing in to start
checks for that session.

## Translators

[Fluent][fluent] is used for localization of the software. Fluent's translation files are found in the [i18n directory](./i18n). New translations may copy the [English (en) localization](./i18n/en) of the project, rename `en` to the desired [ISO 639-1 language code][iso-codes], and then translations can be provided for each [message identifier][fluent-guide]. If no translation is necessary, the message may be omitted.

## Packaging

If packaging for a Linux distribution, vendor dependencies locally with the `vendor` rule, and build with the vendored sources using the `build-vendored` rule. When installing files, use the `rootdir` and `prefix` variables to change installation paths.

```sh
just vendor
just build-vendored
just rootdir=debian/cosmic-ext-mail prefix=/usr install
```

It is recommended to build a source tarball with the vendored dependencies, which can typically be done by running `just vendor` on the host system before it enters the build environment.

## Developers

Developers should install [rustup][rustup] and configure their editor to use [rust-analyzer][rust-analyzer]. To improve compilation times, disable LTO in the release profile, install the [mold][mold] linker, and configure [sccache][sccache] for use with Rust. The [mold][mold] linker will only improve link times if LTO is disabled.

[fluent]: https://projectfluent.org/
[fluent-guide]: https://projectfluent.org/fluent/guide/hello.html
[iso-codes]: https://en.wikipedia.org/wiki/List_of_ISO_639-1_codes
[just]: https://github.com/casey/just
[rustup]: https://rustup.rs/
[rust-analyzer]: https://rust-analyzer.github.io/
[mold]: https://github.com/rui314/mold
[sccache]: https://github.com/mozilla/sccache
