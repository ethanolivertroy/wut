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
      sha256 "632591cdbbd167d96b8052241961d29a719cab82caf0bfd5a11fb91e728fd965"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.1/wut-x86_64-apple-darwin.tar.gz"
      sha256 "5a2fe40466ea7abf586b3fd5e502cd9455fe955cea52ccc79617376e8c791cc0"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.1/wut-aarch64-unknown-linux-musl.tar.gz"
      sha256 "33029f724b1996c98194e6fba4579fa4e31974e6c392124501d9346e8f0e4129"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.1/wut-x86_64-unknown-linux-musl.tar.gz"
      sha256 "2475a43f20d1e906e42959e049307fa289e7436a2eb3b85cd8ba574957debd53"
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
