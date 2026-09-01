class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.11.20260827"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260827/amaru-10.11.20260827-macos-aarch64.tar.gz"
      sha256 "0b47c21243b116c9d354b59e508fb0f3ad67dbe27fe6b8524dd784467cbada20"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260827/amaru-10.11.20260827-linux-aarch64.tar.gz"
      sha256 "57c199ba8f51644d7068568e6f5ffd83800ec7be5af494997deb0e8d524f8400"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260827/amaru-10.11.20260827-linux-x86_64.tar.gz"
      sha256 "1bab9e72b3b5dd2abd6c6752c4a32ab514a58d504bc853b958fa3536dc367990"
    end
  end

  def install
    root = if File.exist?("bin/amaru")
      Pathname.pwd
    else
      candidate = Dir["*/bin/amaru"].find { |entry| File.file?(entry) }
      candidate.nil? ? nil : Pathname.new(candidate).dirname.dirname
    end

    odie "expected extracted Amaru archive contents" if root.nil?

    bin.install root/"bin/amaru"
    man1.install root/"share/man/man1/amaru.1"
    bash_completion.install root/"share/bash-completion/completions/amaru"
    zsh_completion.install root/"share/zsh/site-functions/_amaru"
    fish_completion.install root/"share/fish/vendor_completions.d/amaru.fish"

    docs = root/"share/doc/amaru"
    if docs.directory?
      Dir[docs/"*"].sort.each do |path|
        pkgshare.install path
      end
    end
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/amaru --version")
  end
end
