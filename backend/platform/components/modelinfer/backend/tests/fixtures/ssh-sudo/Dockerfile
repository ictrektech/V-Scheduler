FROM ubuntu:22.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends openssh-server sudo \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd \
    && useradd -m -s /bin/sh admin \
    && useradd -m -s /bin/sh nopass \
    && useradd -m -s /bin/sh denied \
    && useradd -m -s /bin/sh ttyuser \
    && printf 'root:root-test-password\nadmin:sudo-test-password\nnopass:nopass-test-password\ndenied:denied-test-password\nttyuser:tty-test-password\n' | chpasswd \
    && printf 'admin ALL=(root) ALL\nnopass ALL=(root) NOPASSWD: ALL\nttyuser ALL=(root) ALL\nDefaults:ttyuser requiretty\n' > /etc/sudoers.d/modelinfer-test \
    && chmod 0440 /etc/sudoers.d/modelinfer-test \
    && printf 'PermitRootLogin yes\nPasswordAuthentication yes\nUsePAM no\n' > /etc/ssh/sshd_config.d/modelinfer-test.conf
CMD ["/usr/sbin/sshd", "-D", "-e"]
