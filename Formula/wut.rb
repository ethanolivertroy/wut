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
      sha256 "deeba99e7a87a02bd1dde5978a353738017cc848ec96d699c281f4aa258aa977"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.1/wut-x86_64-apple-darwin.tar.gz"
      sha256 "411c7e7c084544183d6dab390f7322167bc8e4ef5c9789042bde85de3de55ea9"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.1/wut-aarch64-unknown-linux-musl.tar.gz"
      sha256 "c2810d2fb29044531883af07ef757f2f0a9490fa7eb0b1ca7cb481f9f0848c7b"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.1/wut-x86_64-unknown-linux-musl.tar.gz"
      sha256 "4eef5e8a799853da66c4b478083d01a055ae092f9843d8f5180d7adb7c26d8c9"
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
