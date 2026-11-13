// Ask the running kernel which ublk features it supports, with
// UBLK_U_CMD_GET_FEATURES on /dev/ublk-control, and print the 64-bit
// mask as hex. Independent of any ublk server, so the probe can check
// a server's own report against it.
//
// Build: cc -O2 -o ublk-features ublk-features.c -luring
// Exit status: 0 and "0x..." on stdout; 1 and the reason on stderr.
#include <errno.h>
#include <fcntl.h>
#include <liburing.h>
#include <linux/ublk_cmd.h>
#include <stdio.h>
#include <string.h>

int main(void)
{
	// The control device takes its commands as io_uring passthrough
	// commands, and their payload needs the 128-byte SQE.
	struct io_uring_params p = { .flags = IORING_SETUP_SQE128 };
	struct io_uring ring;
	struct io_uring_sqe *sqe;
	struct io_uring_cqe *cqe;
	struct ublksrv_ctrl_cmd *cmd;
	__u64 features = 0;
	int fd, ret;

	fd = open("/dev/ublk-control", O_RDWR);
	if (fd < 0) {
		fprintf(stderr, "open /dev/ublk-control: %s\n", strerror(errno));
		return 1;
	}
	ret = io_uring_queue_init_params(4, &ring, &p);
	if (ret < 0) {
		fprintf(stderr, "io_uring_queue_init_params: %s\n", strerror(-ret));
		return 1;
	}
	sqe = io_uring_get_sqe(&ring);
	if (!sqe) {
		fprintf(stderr, "io_uring_get_sqe: no entry\n");
		return 1;
	}
	// 128 bytes: the SQE and the command area behind it.
	memset((void *)sqe, 0, 2 * sizeof(*sqe));
	sqe->opcode = IORING_OP_URING_CMD;
	sqe->fd = fd;
	sqe->cmd_op = UBLK_U_CMD_GET_FEATURES;
	cmd = (struct ublksrv_ctrl_cmd *)sqe->cmd;
	cmd->dev_id = (__u32)-1;
	cmd->queue_id = (__u16)-1;
	cmd->len = UBLK_FEATURES_LEN;
	cmd->addr = (__u64)(unsigned long)&features;

	ret = io_uring_submit_and_wait(&ring, 1);
	if (ret < 0) {
		fprintf(stderr, "io_uring_submit_and_wait: %s\n", strerror(-ret));
		return 1;
	}
	ret = io_uring_wait_cqe(&ring, &cqe);
	if (ret < 0) {
		fprintf(stderr, "io_uring_wait_cqe: %s\n", strerror(-ret));
		return 1;
	}
	if (cqe->res < 0) {
		fprintf(stderr, "UBLK_U_CMD_GET_FEATURES: %s\n", strerror(-cqe->res));
		return 1;
	}
	printf("0x%llx\n", (unsigned long long)features);
	return 0;
}
