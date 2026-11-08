class Wut < Formula
  desc "Tiny, fast terminal assistant powered by Cerebras or Groq"
  homepage "https://github.com/ethanolivertroy/wut"
  version "0.0.4"
  license "MIT"

  livecheck do
    url :stable
    strategy :github_latest
  end

  on_macos do
    on_arm do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.4/wut-aarch64-apple-darwin.tar.gz"
      sha256 "f55df6d8cf1cf748f0988d0fe2d1014610f7d809304ab5fdb2cb95b7949c4b40"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.4/wut-x86_64-apple-darwin.tar.gz"
      sha256 "e11af5c772b33e9705fa951b726df1f5d0448f7830668ce0e50e92c69f0921df"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.4/wut-aarch64-unknown-linux-musl.tar.gz"
      sha256 "98a1b7fe1fa4b21ed8bacc2ba2bdeeef9b452a8af5ffc884fc20dbcccb1ee6b0"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.4/wut-x86_64-unknown-linux-musl.tar.gz"
      sha256 "0176ae37af486fdb92dfa1c0d66310d188a883db6fea1bb5fae6cb296a30be97"
    end
  end

  def install
    bin.install "wut"
    pkgshare.install "README.md", "LICENSE"
  end

  def caveats
    <<~EOS
      Set a provider API key in your process environment:
        export CEREBRAS_API_KEY=...
      Or use Groq:
        export GROQ_API_KEY=...
        wut --model groq:openai/gpt-oss-20b "your question"

      Optional Cerebras storage is plaintext, not encrypted:
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
