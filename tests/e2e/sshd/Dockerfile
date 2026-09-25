# Throwaway OpenSSH server for the E2E harness and the #[ignore] mount tests (contract §12).
# Host keys are generated at build time, so they survive `docker stop`/`docker start`.
FROM alpine:3.20
RUN apk add --no-cache openssh-server openssh-sftp-server \
 && ssh-keygen -A \
 && adduser -D bf \
 && echo 'bf:*' | chpasswd -e \
 && mkdir -p /home/bf/data /home/bf/.ssh \
 && echo hello > /home/bf/hello.txt \
 && echo w > /home/bf/data/w.txt \
 && chmod 700 /home/bf/.ssh \
 && chown -R bf:bf /home/bf
# $PUBKEY (docker run -e) becomes the only authorized key; the password is locked ('*').
CMD printf '%s\n' "$PUBKEY" > /home/bf/.ssh/authorized_keys \
 && chmod 600 /home/bf/.ssh/authorized_keys \
 && chown bf:bf /home/bf/.ssh/authorized_keys \
 && exec /usr/sbin/sshd -D -e
