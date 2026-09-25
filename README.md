# AirCard

AirCard is a macOS app for changing Apple Wallet card artwork on a connected iPhone. The current app uses Tauri, React and Rust. It requires macOS 14 or later and a trusted iPhone connected over USB.

## Use

1. Connect and unlock the iPhone. Trust this Mac when prompted.
2. Choose **扫描卡片**. On the iPhone, open Wallet and switch through the cards you want to find.
3. Click the cards to select targets and choose an image in the panel on the right. Everything stays on one page.
4. Choose **刷入 iPhone**. AirCard saves missing originals before changing card artwork.

The **更多操作** menu on each card lets you read its current artwork, save its original image, restore that image, or remove the card from the local list. The top-right **维护** button opens cleanup, local file locations and logs.

The scanner depends on iPhone logs. A card that has not appeared in those logs may need to be opened in Wallet before AirCard can find it. See [scanner validation](docs/wallet-card-detection.md).

## Download and build

GitHub Actions builds a universal macOS app for Apple Silicon and Intel on pull requests, pushes to main, version tags and manual runs. npm and Rust dependencies are cached; ThinLTO and parallel code generation shorten linking, and already compressed archives are uploaded without recompression. Download the DMG or ZIP from the **Package macOS** workflow artifacts. Builds without a Developer ID are signed ad hoc and are not notarized.

To build locally:

~~~sh
npm ci
make test
npm run test:ui
make bundle
~~~

The signed app is packaged into a DMG and ZIP under target/release/bundle/dmg/. The make bundle command signs the app before creating either archive. Set AIR_CARD_IDENTITY to a Developer ID signing identity to use that identity instead of an ad hoc signature.

The UI tests use an installed Google Chrome and a mocked device; they never write to an iPhone. For development, use make dev. The make test-legacy command runs the remaining Python reference tests; the old Swift app and its build script have been removed.

## Credits

The device communication and AirTraffic work builds on [AirLift](https://github.com/0xjohnnydev/airlift) by 0xjohnny. Earlier AirCard development was led by [Mak5er](https://github.com/Mak5er) and [Lumid-Off](https://github.com/Lumid-Off).

Licensed under [MIT](LICENSE).
