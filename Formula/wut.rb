class Wut < Formula
  desc "Tiny, fast terminal assistant powered by Cerebras"
  homepage "https://github.com/ethanolivertroy/wut"
  version "0.0.5"
  license "MIT"

  livecheck do
    url :stable
    strategy :github_latest
  end

  on_macos do
    on_arm do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.5/wut-aarch64-apple-darwin.tar.gz"
      sha256 "7637f3ba7743efdbf029341458de89b82799026c5b9f95fd9b4aff0ba65366eb"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.5/wut-x86_64-apple-darwin.tar.gz"
      sha256 "15bf0fb7ed036a3bffa0ef86bfe8eb29cd921e03f972b48578b815a85a6a62c6"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.5/wut-aarch64-unknown-linux-musl.tar.gz"
      sha256 "3633e7b9d0f2efc475498a8c5fde272860522a1888d23b5a7de7865d2a7b968b"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.5/wut-x86_64-unknown-linux-musl.tar.gz"
      sha256 "d85a4652b3cbcabfcc650359ecf74e6427b116cb7c242c5f68e025b765648513"
    end
  end

  def install
    bin.install "wut"
    pkgshare.install "README.md", "LICENSE"
  end

  def caveats
    <<~EOS
      wut needs a Cerebras API key. Export it:
        export CEREBRAS_API_KEY=...

      Or save it once, the way the official installer does:
        mkdir -p ~/.config/wut && chmod 700 ~/.config/wut
        printf '%s\\n' YOUR_KEY > ~/.config/wut/credentials
        chmod 600 ~/.config/wut/credentials

      Optional web search: export EXA_API_KEY=...

      For zsh, the official installer also adds punctuation support:
        alias wut='noglob command wut'
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/wut --version")
  end
end
