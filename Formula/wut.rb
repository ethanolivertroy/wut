class Wut < Formula
  desc "Tiny, fast terminal assistant"
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
      sha256 "3fe7e5075106d62f78bbfd5d2eb82e4e606c4446424907e2e0d76f396607c207"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.5/wut-x86_64-apple-darwin.tar.gz"
      sha256 "96562e4f279ad4a9555080967071236189f6e6f5ab527306e474325ee9d3d20f"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.5/wut-aarch64-unknown-linux-musl.tar.gz"
      sha256 "f94db4a3cd20ffab69bbe2d1bc3535d623a100383f2681034c18fcabcb063beb"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.5/wut-x86_64-unknown-linux-musl.tar.gz"
      sha256 "cc08c878621b0b363f381ef1c79f04a92fdcf1ba7057c0122ba59cea196be37a"
    end
  end

  def install
    bin.install "wut"
    pkgshare.install "README.md", "LICENSE"
  end

  def caveats
    <<~EOS
      Save a provider key with hidden terminal input:
        wut --set-key cerebras
        wut --set-key groq
        wut --set-key ai-gateway
      Run wut --set-key to choose a provider. Do not pass the key as an argument.

      Choose a model in wut --settings, or pass its ID:
        wut --model groq:openai/gpt-oss-20b "your question"

      Saved keys are plaintext, not encrypted or stored in an OS keychain.
      Key files use owner-only permissions (0600), in private directories (0700).
      Loading rejects unsafe permissions/ownership, symlinks, hard links, and
      macOS extended ACLs. CEREBRAS_API_KEY, GROQ_API_KEY, and AI_GATEWAY_API_KEY
      environment variables override saved credentials for their provider.

      Optional web search: export EXA_API_KEY=...

      For zsh, the official installer also adds punctuation support:
        alias wut='noglob command wut'
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/wut --version")
  end
end
