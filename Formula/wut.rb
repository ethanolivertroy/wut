class Wut < Formula
  desc "Tiny, fast terminal assistant powered by Cerebras"
  homepage "https://github.com/ethanolivertroy/wut"
  version "0.0.3"
  license "MIT"

  livecheck do
    url :stable
    strategy :github_latest
  end

  on_macos do
    on_arm do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.3/wut-aarch64-apple-darwin.tar.gz"
      sha256 "1a2387c2c4ab72aad2df59ce58ce0d6f74cca0057d703534549245ca1d2c02fd"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.3/wut-x86_64-apple-darwin.tar.gz"
      sha256 "6b1f8de700b2f6de88c3da06b11e63008a5fd35b05fabed06a619701374e54cc"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.3/wut-aarch64-unknown-linux-musl.tar.gz"
      sha256 "3fecb3888db736af227b61f146ec22941eac855ab9d60f0b6feb593fad1176d2"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.3/wut-x86_64-unknown-linux-musl.tar.gz"
      sha256 "386736e059f03240bc67891618d813d5b35933876ad3d1dcedb31c1de95b2d72"
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
