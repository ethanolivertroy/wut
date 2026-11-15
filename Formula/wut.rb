class Wut < Formula
  desc "Tiny, fast terminal assistant"
  homepage "https://github.com/ethanolivertroy/wut"
  version "0.0.6"
  license "MIT"

  livecheck do
    url :stable
    strategy :github_latest
  end

  on_macos do
    on_arm do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.6/wut-aarch64-apple-darwin.tar.gz"
      sha256 "4f6afc1ad335e10ab822544f5f31b8c7a21990713674af109f9603792e2ca142"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.6/wut-x86_64-apple-darwin.tar.gz"
      sha256 "f6e474223722446a21371cf3425f7ca047b4de84ce663d403d43fc99b5c1b48e"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.6/wut-aarch64-unknown-linux-musl.tar.gz"
      sha256 "ee037a940abbdfdf2708a493e1de2573b5b9d9db57d1e89d878878b44b3e6a1a"
    end
    on_intel do
      url "https://github.com/ethanolivertroy/wut/releases/download/v0.0.6/wut-x86_64-unknown-linux-musl.tar.gz"
      sha256 "a5abacb889629936dc134da59b1d982261ea7035201c25fa99728815046ee2e0"
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
