class Wut < Formula
  desc "Tiny, fast terminal assistant powered by Cerebras"
  homepage "https://github.com/ethanolivertroy/wut"
  version "0.0.1"
  license "MIT"

  livecheck do
    url :stable
    strategy :github_latest
  end

  on_macos do
    on_arm do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.1/wut-aarch64-apple-darwin.tar.gz"
      sha256 "7ed038a7d126069a0c54bd1452485ec57c69043dd620355aff1e97deb9cfabb4"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.1/wut-x86_64-apple-darwin.tar.gz"
      sha256 "c18389481ffc2d7f75cc2ed5bf0e6f686f67273443470b8cf2b68bdc60e201f1"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.1/wut-aarch64-unknown-linux-musl.tar.gz"
      sha256 "8dc1f7de13cacfe6ae7b313562ae6fbfdd0f46ff4b9ef16fc5a0d45b97725074"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.1/wut-x86_64-unknown-linux-musl.tar.gz"
      sha256 "ae89bfefc0c4e14f59851545f59db6d4f8f2024b80a7231741c27f1e8d968b79"
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
