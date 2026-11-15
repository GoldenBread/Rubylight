// resolve-probe.patch: probeAddressPorts tests every test port of the addresses together and
// takes the earliest address that answers on any of them; resolveHostNameOnPorts takes a single
// address untested. Platform.c and PlatformSockets.c are the real ones; the sockets are loopback.
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include "Limelight-internal.h"
// After Platform.h, which defines NDEBUG without LC_DEBUG.
#undef NDEBUG
#include <assert.h>

CONNECTION_LISTENER_CALLBACKS ListenerCallbacks;
STREAM_CONFIGURATION StreamConfig;
int AppVersionQuad[4];
struct sockaddr_storage RemoteAddr;
struct sockaddr_storage LocalAddr;
SOCKADDR_LEN AddrLen;
uint16_t RtspPortNumber;

static void logMessage(const char* format, ...) {
    va_list args;
    va_start(args, format);
    vfprintf(stderr, format, args);
    va_end(args);
}

// Platform.c starts ENet with the platform; this test never uses it.
int enet_initialize(void) { return 0; }
void enet_deinitialize(void) {}

static struct sockaddr_in loopback(const char* ip) {
    struct sockaddr_in a;
    memset(&a, 0, sizeof(a));
    a.sin_family = AF_INET;
    assert(inet_pton(AF_INET, ip, &a.sin_addr) == 1);
    return a;
}

// A TCP socket on 127.0.0.1 and its port: listening, or closed again for a port that refuses.
static unsigned short loopbackPort(bool listening, SOCKET* kept) {
    struct sockaddr_in a = loopback("127.0.0.1");
    socklen_t len = sizeof(a);
    SOCKET s = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    assert(s != INVALID_SOCKET);
    assert(bind(s, (struct sockaddr*)&a, sizeof(a)) == 0);
    assert(getsockname(s, (struct sockaddr*)&a, &len) == 0);
    if (listening) {
        assert(listen(s, 8) == 0);
        *kept = s;
    }
    else {
        closeSocket(s);
    }
    return ntohs(a.sin_port);
}

static void entry(struct addrinfo* info, struct sockaddr_in* address, struct addrinfo* next) {
    memset(info, 0, sizeof(*info));
    info->ai_family = AF_INET;
    info->ai_socktype = SOCK_STREAM;
    info->ai_protocol = IPPROTO_TCP;
    info->ai_addr = (struct sockaddr*)address;
    info->ai_addrlen = sizeof(*address);
    info->ai_next = next;
}

int main(void) {
    ListenerCallbacks.logMessage = logMessage;
    initializePlatformSockets();
    SOCKET listener = INVALID_SOCKET;
    unsigned short answering = loopbackPort(true, &listener);
    unsigned short closedA = loopbackPort(false, NULL);
    unsigned short closedB = loopbackPort(false, NULL);

    // The address answers on its last test port only: it works, as when 48010 alone listens.
    struct sockaddr_in local = loopback("127.0.0.1");
    struct addrinfo only;
    entry(&only, &local, NULL);
    unsigned short ports[] = { closedA, closedB, answering };
    assert(probeAddressPorts(&only, ports, 3) == 0);

    // No port answers: no address, and the refusals end the test at once, not one port after another.
    uint64_t started = PltGetMillis();
    assert(probeAddressPorts(&only, ports, 2) == -1);
    assert(PltGetMillis() - started < 1500);

    // An earlier address that never answers (TEST-NET-1) gets the preference wait, not a timeout
    // per port: the later address that answers is taken within about 250 ms.
    struct sockaddr_in dead = loopback("192.0.2.1");
    struct addrinfo second, first;
    entry(&second, &local, NULL);
    entry(&first, &dead, &second);
    started = PltGetMillis();
    assert(probeAddressPorts(&first, ports, 3) == 1);
    assert(PltGetMillis() - started < 1500);

    // A single address is taken untested, whatever its ports.
    struct sockaddr_storage address;
    SOCKADDR_LEN length = 0;
    assert(resolveHostNameOnPorts("127.0.0.1", AF_UNSPEC, ports, 2, &address, &length) == 0);
    assert(address.ss_family == AF_INET && length == sizeof(struct sockaddr_in));

    closeSocket(listener);
    cleanupPlatformSockets();
    return 0;
}
