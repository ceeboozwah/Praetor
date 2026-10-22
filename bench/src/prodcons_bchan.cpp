// bench/prodcons_bchan — N heap-allocated packets through a BOUNDED
// channel (capacity argv[2]), producer thread -> main consumer.
// malloc ownership crosses the send; the consumer frees each packet.
// Track-B idiomatic C++: bounded queue = ring + std::mutex + two
// std::condition_variable (the stdlib sync facility, not pthread calls).
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <mutex>
#include <thread>
#include <vector>

struct Node { int64_t v; };

class Chan {
public:
    explicit Chan(int64_t cap)
        : ring_((size_t)cap), cap_(cap) {}

    void send(Node *m) {
        std::unique_lock<std::mutex> lk(mu_);
        not_full_.wait(lk, [&] { return count_ < cap_; });
        ring_[tail_] = m;
        tail_ = (tail_ + 1) % cap_;
        ++count_;
        not_empty_.notify_one();
    }

    Node *recv() {
        std::unique_lock<std::mutex> lk(mu_);
        not_empty_.wait(lk, [&] { return count_ > 0; });
        Node *m = ring_[head_];
        head_ = (head_ + 1) % cap_;
        --count_;
        not_full_.notify_one();
        return m;
    }

private:
    std::mutex mu_;
    std::condition_variable not_full_, not_empty_;
    std::vector<Node *> ring_;
    int64_t cap_, count_ = 0, head_ = 0, tail_ = 0;
};

int main(int argc, char **argv) {
    if (argc != 3) return 1;
    int64_t n = strtoll(argv[1], 0, 10);
    int64_t cap = strtoll(argv[2], 0, 10);
    Chan ch(cap);
    std::thread t([&] {
        for (int64_t i = 0; i < n; i++) {
            Node *m = (Node *)malloc(sizeof(Node));
            m->v = i;
            ch.send(m);
        }
        Node *stop = (Node *)malloc(sizeof(Node));
        stop->v = -1;
        ch.send(stop);
    });
    int64_t total = 0;
    for (;;) {
        Node *g = ch.recv();
        if (g->v < 0) { free(g); break; }
        total += g->v;
        free(g);
    }
    t.join();
    printf("%lld\n", (long long)total);
    return 0;
}
