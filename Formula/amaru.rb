class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.11.20261105"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20261105/amaru-10.11.20261105-macos-aarch64.tar.gz"
      sha256 "bca341191f567f24b909af7196d0a4de1143580d0a89ec9e718540d845450339"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20261105/amaru-10.11.20261105-linux-aarch64.tar.gz"
      sha256 "801f895727cefefafc690b3f228fd033ab28683a1f5bde39a7070b8b79ac4e15"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20261105/amaru-10.11.20261105-linux-x86_64.tar.gz"
      sha256 "a2c00d25b58dde909673da8be20c88da8957aabdbaa1202b7ce7cb919afc0e74"
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
