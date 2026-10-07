// Native objects, callback and exception handling stay on this side of the ABI.
// This shim deliberately counts cooperative terminator polls, not elapsed time
// or the amount of primitive work between polls.
#include <bitwuzla/cpp/bitwuzla.h>
#include <bitwuzla/cpp/terminator.h>

#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <limits>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace {

struct TermRef {
  std::uint64_t owner;
  std::uint64_t id;
};

class PollTerminator final : public bitwuzla::Terminator {
 public:
  explicit PollTerminator(std::uint32_t limit) : limit_(limit) {}

  void start() noexcept {
    polls_ = 0;
    exhausted_ = false;
  }

  bool terminate() override {
    if (exhausted_) return true;
    ++polls_;
    exhausted_ = polls_ >= limit_;
    return exhausted_;
  }

  std::uint64_t polls() const noexcept { return polls_; }
  bool exhausted() const noexcept { return exhausted_; }

 private:
  const std::uint32_t limit_;
  std::uint64_t polls_ = 0;
  bool exhausted_ = false;
};

bitwuzla::Options configured_options() {
  bitwuzla::Options options;
  options.set(bitwuzla::Option::PRODUCE_MODELS, 1);
  options.set(bitwuzla::Option::NTHREADS, 1);
  options.set(bitwuzla::Option::BV_SOLVER, "bitblast");
  options.set(bitwuzla::Option::SAT_SOLVER, "cadical");
  // Do not configure TIME_LIMIT_PER. Its native default is zero (disabled).
  return options;
}

struct Session {
  const std::uint64_t owner;
  bitwuzla::TermManager manager;
  bitwuzla::Options options;
  // Destruction order keeps this terminator and manager alive for the solver.
  PollTerminator terminator;
  bitwuzla::Bitwuzla solver;
  std::vector<bitwuzla::Term> terms;
  std::string model;
  char error[2048] = {};
  bool had_sat = false;
  bool poisoned = false;

  Session(std::uint64_t owner_id, std::uint32_t limit)
      : owner(owner_id), options(configured_options()), terminator(limit),
        solver(manager, options) {
    solver.configure_terminator(&terminator);
  }

  const bitwuzla::Term& term(TermRef ref) const {
    if (ref.owner != owner) {
      throw std::invalid_argument("term belongs to another solver session");
    }
    if (ref.id >= terms.size()) {
      throw std::invalid_argument("term ID is outside its solver session");
    }
    return terms[static_cast<std::size_t>(ref.id)];
  }

  std::uint64_t store(bitwuzla::Term term) {
    if (terms.size() == std::numeric_limits<std::uint64_t>::max()) {
      throw std::overflow_error("native term ID limit reached");
    }
    const auto id = static_cast<std::uint64_t>(terms.size());
    terms.push_back(std::move(term));
    return id;
  }
};

thread_local char creation_error[2048] = {};

void error_message(char* buffer, std::size_t size, const char* message) noexcept {
  std::snprintf(buffer, size, "%s", message);
}

template <typename Operation>
int guarded(Session* session, Operation operation) noexcept {
  if (session == nullptr) return 0;
  session->error[0] = '\0';
  try {
    if (session->poisoned) {
      throw std::invalid_argument("native session unusable after a solver exception");
    }
    operation(*session);
    return 1;
  } catch (const std::invalid_argument& ex) {
    // Shim validation has not changed the native solver state.
    error_message(session->error, sizeof(session->error), ex.what());
  } catch (const std::exception& ex) {
    // Bitwuzla documents native API exceptions as potentially invalidating the
    // object. Do not allow reuse after one, including allocation failures.
    session->poisoned = true;
    session->had_sat = false;
    error_message(session->error, sizeof(session->error), ex.what());
  } catch (...) {
    session->poisoned = true;
    session->had_sat = false;
    error_message(session->error, sizeof(session->error), "non-standard native exception");
  }
  return 0;
}

void arity(const std::vector<bitwuzla::Term>& args, std::size_t expected) {
  if (args.size() != expected) throw std::invalid_argument("invalid operation arity");
}

void booleans(const std::vector<bitwuzla::Term>& args) {
  for (const auto& term : args) {
    if (!term.sort().is_bool()) throw std::invalid_argument("expected Boolean argument");
  }
}

void bit_vectors(const std::vector<bitwuzla::Term>& args) {
  for (const auto& term : args) {
    if (!term.sort().is_bv()) throw std::invalid_argument("expected bit-vector argument");
    if (term.sort() != args.front().sort()) {
      throw std::invalid_argument("bit-vector argument widths differ");
    }
  }
}

unsigned digit(char value) {
  if (value >= '0' && value <= '9') return static_cast<unsigned>(value - '0');
  if (value >= 'a' && value <= 'f') return static_cast<unsigned>(value - 'a' + 10);
  if (value >= 'A' && value <= 'F') return static_cast<unsigned>(value - 'A' + 10);
  throw std::invalid_argument("invalid hexadecimal bit-vector digit");
}

void check_hex(std::uint32_t width, const char* hex) {
  if (width == 0) throw std::invalid_argument("bit-vector width must be positive");
  if (hex == nullptr || hex[0] == '\0') throw std::invalid_argument("empty hexadecimal value");
  std::size_t leading = 0, length = 0;
  while (hex[length] != '\0') {
    (void) digit(hex[length]);
    if (hex[length] == '0' && leading == length) ++leading;
    ++length;
  }
  const auto significant = length - leading;
  const auto digits = (static_cast<std::uint64_t>(width) + 3) / 4;
  if (significant > digits ||
      (significant == digits && width % 4 != 0 &&
       digit(hex[leading]) >= (1u << (width % 4)))) {
    throw std::invalid_argument("hexadecimal value exceeds bit-vector width");
  }
}

bitwuzla::Kind binary_kind(std::uint32_t kind) {
  switch (kind) {
    case 5: return bitwuzla::Kind::BV_ADD;
    case 6: return bitwuzla::Kind::BV_SUB;
    case 7: return bitwuzla::Kind::BV_MUL;
    case 8: return bitwuzla::Kind::BV_UDIV;
    case 9: return bitwuzla::Kind::BV_SDIV;
    case 10: return bitwuzla::Kind::BV_UREM;
    case 11: return bitwuzla::Kind::BV_SREM;
    case 12: return bitwuzla::Kind::BV_ULT;
    case 13: return bitwuzla::Kind::BV_ULE;
    case 14: return bitwuzla::Kind::BV_UGT;
    case 15: return bitwuzla::Kind::BV_UGE;
    case 16: return bitwuzla::Kind::BV_SLT;
    case 17: return bitwuzla::Kind::BV_SLE;
    case 18: return bitwuzla::Kind::BV_SGT;
    case 19: return bitwuzla::Kind::BV_SGE;
    case 20: return bitwuzla::Kind::BV_AND;
    case 21: return bitwuzla::Kind::BV_OR;
    case 22: return bitwuzla::Kind::BV_XOR;
    case 24: return bitwuzla::Kind::BV_SHL;
    case 25: return bitwuzla::Kind::BV_SHR;
    case 26: return bitwuzla::Kind::BV_ASHR;
    default: throw std::invalid_argument("unknown bit-vector operation");
  }
}

bitwuzla::Term apply(Session& session, std::uint32_t kind,
                      const std::vector<bitwuzla::Term>& args,
                      const std::vector<std::uint64_t>& indices) {
  if (kind != 27 && kind != 28 && !indices.empty()) {
    throw std::invalid_argument("unexpected operation indices");
  }
  switch (kind) {
    case 0:
      arity(args, 1);
      booleans(args);
      return session.manager.mk_term(bitwuzla::Kind::NOT, args);
    case 1:
    case 2:
      booleans(args);
      if (args.empty()) return kind == 1 ? session.manager.mk_true() : session.manager.mk_false();
      if (args.size() == 1) return args[0];
      return session.manager.mk_term(kind == 1 ? bitwuzla::Kind::AND : bitwuzla::Kind::OR, args);
    case 3:
      arity(args, 2);
      if (args[0].sort() != args[1].sort()) throw std::invalid_argument("equality sorts differ");
      return session.manager.mk_term(bitwuzla::Kind::EQUAL, args);
    case 4:
      arity(args, 3);
      if (!args[0].sort().is_bool() || args[1].sort() != args[2].sort()) {
        throw std::invalid_argument("invalid conditional sorts");
      }
      return session.manager.mk_term(bitwuzla::Kind::ITE, args);
    case 23:
      arity(args, 1);
      bit_vectors(args);
      return session.manager.mk_term(bitwuzla::Kind::BV_NOT, args);
    case 27:
      arity(args, 1);
      bit_vectors(args);
      if (indices.size() != 1 ||
          args[0].sort().bv_size() + indices[0] > std::numeric_limits<std::uint32_t>::max()) {
        throw std::invalid_argument("invalid zero extension index");
      }
      return session.manager.mk_term(bitwuzla::Kind::BV_ZERO_EXTEND, args, indices);
    case 28:
      arity(args, 1);
      bit_vectors(args);
      if (indices.size() != 2 || indices[0] < indices[1] ||
          indices[0] >= args[0].sort().bv_size()) {
        throw std::invalid_argument("invalid bit extraction indices");
      }
      return session.manager.mk_term(bitwuzla::Kind::BV_EXTRACT, args, indices);
    default:
      arity(args, 2);
      bit_vectors(args);
      return session.manager.mk_term(binary_kind(kind), args);
  }
}

} // namespace

extern "C" {

void* evmbw_new(std::uint64_t owner, std::uint32_t rlimit) noexcept {
  creation_error[0] = '\0';
  try {
    if (owner == 0 || rlimit == 0) throw std::invalid_argument("owner and rlimit must be positive");
    return new Session(owner, rlimit);
  } catch (const std::exception& ex) {
    error_message(creation_error, sizeof(creation_error), ex.what());
  } catch (...) {
    error_message(creation_error, sizeof(creation_error), "non-standard native construction exception");
  }
  return nullptr;
}

const char* evmbw_creation_error() noexcept { return creation_error; }

void evmbw_delete(void* opaque) noexcept {
  // Public destructors are noexcept; no C++ exception may cross the C ABI.
  try { delete static_cast<Session*>(opaque); } catch (...) {}
}

const char* evmbw_error(void* opaque) noexcept {
  return opaque == nullptr ? "null native session" : static_cast<Session*>(opaque)->error;
}

int evmbw_bool(void* opaque, std::uint8_t value, std::uint64_t* out) noexcept {
  return guarded(static_cast<Session*>(opaque), [&](Session& session) {
    if (out == nullptr || value > 1) throw std::invalid_argument("invalid Boolean output or value");
    *out = session.store(value ? session.manager.mk_true() : session.manager.mk_false());
  });
}

int evmbw_bv(void* opaque, std::uint32_t width, const char* hex,
             std::uint64_t* out) noexcept {
  return guarded(static_cast<Session*>(opaque), [&](Session& session) {
    if (out == nullptr) throw std::invalid_argument("null term output");
    check_hex(width, hex);
    *out = session.store(session.manager.mk_bv_value(session.manager.mk_bv_sort(width), hex, 16));
  });
}

int evmbw_variable(void* opaque, std::uint32_t width, const char* name,
                   std::uint64_t* out) noexcept {
  return guarded(static_cast<Session*>(opaque), [&](Session& session) {
    if (out == nullptr || name == nullptr) throw std::invalid_argument("null variable input or output");
    auto sort = width == 0 ? session.manager.mk_bool_sort() : session.manager.mk_bv_sort(width);
    *out = session.store(session.manager.mk_const(sort, std::string(name)));
  });
}

int evmbw_apply(void* opaque, std::uint32_t kind, const TermRef* args,
                std::size_t argc, const std::uint32_t* indices,
                std::size_t indexc, std::uint64_t* out) noexcept {
  return guarded(static_cast<Session*>(opaque), [&](Session& session) {
    if (out == nullptr || (argc != 0 && args == nullptr) ||
        (indexc != 0 && indices == nullptr)) {
      throw std::invalid_argument("null operation input or output");
    }
    std::vector<bitwuzla::Term> terms;
    terms.reserve(argc);
    for (std::size_t i = 0; i < argc; ++i) terms.push_back(session.term(args[i]));
    std::vector<std::uint64_t> native_indices;
    native_indices.reserve(indexc);
    for (std::size_t i = 0; i < indexc; ++i) native_indices.push_back(indices[i]);
    *out = session.store(apply(session, kind, terms, native_indices));
  });
}

int evmbw_assert(void* opaque, TermRef ref) noexcept {
  return guarded(static_cast<Session*>(opaque), [&](Session& session) {
    const auto& term = session.term(ref);
    if (!term.sort().is_bool()) throw std::invalid_argument("assertion must have Boolean sort");
    session.had_sat = false;
    session.solver.assert_formula(term);
  });
}

int evmbw_check(void* opaque, std::uint32_t* status, std::uint8_t* exhausted,
                std::uint64_t* polls) noexcept {
  return guarded(static_cast<Session*>(opaque), [&](Session& session) {
    if (status == nullptr || exhausted == nullptr || polls == nullptr) {
      throw std::invalid_argument("null check output");
    }
    session.had_sat = false;
    session.terminator.start();
    const auto result = session.solver.check_sat();
    *status = result == bitwuzla::Result::SAT ? 10 : result == bitwuzla::Result::UNSAT ? 20 : 0;
    *exhausted = session.terminator.exhausted() ? 1 : 0;
    *polls = session.terminator.polls();
    session.had_sat = result == bitwuzla::Result::SAT;
  });
}

int evmbw_model(void* opaque, TermRef ref, const char** out) noexcept {
  return guarded(static_cast<Session*>(opaque), [&](Session& session) {
    if (out == nullptr) throw std::invalid_argument("null model output");
    const auto& term = session.term(ref);
    if (!session.had_sat) throw std::invalid_argument("model requires the latest check to be SAT");
    const auto value = session.solver.get_value(term);
    if (!value.is_value()) throw std::runtime_error("native model value is not concrete");
    if (value.sort().is_bool()) {
      session.model = value.value<bool>() ? "#b1" : "#b0";
    } else if (value.sort().is_bv()) {
      session.model = "#b" + value.value<std::string>(2);
    } else {
      throw std::runtime_error("unsupported native model sort");
    }
    *out = session.model.c_str();
  });
}

} // extern C
