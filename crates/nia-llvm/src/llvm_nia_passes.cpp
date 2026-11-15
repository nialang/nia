// SPDX-License-Identifier: GPL-3.0-or-later
//
// Nia's own IR passes, which every pipeline the bridges build adds to LLVM's
// default pipeline (see `niaRegisterPasses` at the end). Both decide
// comparisons that checked code repeats and LLVM cannot decide, so that the
// checks they guard fold away.
//
// MergeImpliedConditions decides comparisons that hold on every path to them
// without holding on any single dominating branch. LLVM proves a comparison
// from conditions that dominate it; checked code also produces comparisons of
// values that are phis of merges, where each incoming path has proven the
// comparison for the value it brings and no condition covers them all. A
// capacity check that grows the storage on its rare path and checks again,
// then writes an element whose index the write checks once more, has that
// shape: the write's check stays, although both paths have proven it.
//
// `holdsAt` asks whether `pred lhs, rhs` is decided at the end of a block. It
// first looks for a branch that dominates the block and implies the
// comparison. Failing that, it splits on a phi: it takes the operand that is
// a phi of the nearest merge dominating the block, translates both operands
// to the values each edge into that merge carries, and asks the same question
// of each edge, recursively. The comparison is decided when every edge
// decides it the same way; because SSA values do not change between the
// merge and the block, it then holds at the block too. A loop header is a
// merge like any other: its entry edge and its back edge are each proven
// from their own facts, never from what is being proven. A work budget
// bounds every query.
//
// The branches between the merge and the block hold at the block, and are
// translated to each edge as the operands are. An edge on which one of them
// is false cannot reach the block and is not asked; one on which it is not
// decided carries it as a fact to the edge's own question. A loop that keeps
// in a flag what its last iteration did is the usual case: the work that
// only some iterations leave to the next is guarded by the flag, and the
// iterations that leave none set it false.
//
// The implication between conditions is LLVM's `isImpliedCondition`, which
// relates two values to each other and not only to constants, so `i < n`
// decides `i < n` and `i <= n` on the edge it guards.

#include "llvm_nia.h"

#include "llvm/Analysis/InstructionSimplify.h"
#include "llvm/Analysis/ValueTracking.h"
#include "llvm/IR/CFG.h"
#include "llvm/IR/Constants.h"
#include "llvm/IR/Dominators.h"
#include "llvm/IR/Function.h"
#include "llvm/IR/Instructions.h"
#include "llvm/IR/PassManager.h"
#include "llvm/IR/PatternMatch.h"
#include "llvm/Passes/PassBuilder.h"
#include "llvm/Transforms/Scalar/SimplifyCFG.h"

#include <optional>

using namespace llvm;

namespace {

// Bounds on one query: how many dominating branches are consulted above a
// block, how many predecessors a merge may have, how many instructions of a
// merge a value is translated through, how deeply phis are split, and how
// many edges one query may examine in all.
constexpr unsigned MaxDominatorDepth = 16;
constexpr unsigned MaxPredecessors = 8;
constexpr unsigned MaxTranslationDepth = 4;
constexpr unsigned MaxSplitDepth = 6;
constexpr unsigned MaxEdgesPerQuery = 48;
constexpr unsigned MaxResolutionDepth = 3;

// Whether `inst` reads one operand into a value of the same meaning: a field
// extraction or a cast, through which a value of a merge is translated.
bool isProjection(const Instruction *I) {
  return isa<ExtractValueInst>(I) || isa<CastInst>(I);
}

// `v`, or the value a phi is when every edge brings that same value, as a
// phi that closes a loop's value at its exit does. That value dominates the
// phi, so facts proven about it hold for the phi.
Value *throughTrivialPhis(Value *V) {
  for (unsigned Depth = 0; Depth < MaxTranslationDepth; ++Depth) {
    auto *Phi = dyn_cast<PHINode>(V);
    if (!Phi || Phi->getNumIncomingValues() == 0)
      return V;
    Value *Same = Phi->getIncomingValue(0);
    if (Same == Phi ||
        !all_of(Phi->incoming_values(), [&](Value *In) { return In == Same; }))
      return V;
    V = Same;
  }
  return V;
}

// The phi a value projects, when it is a phi or a chain of projections of
// one within the phi's block.
PHINode *projectedPhi(Value *V) {
  auto *I = dyn_cast<Instruction>(V);
  for (unsigned Depth = 0; I && Depth < MaxTranslationDepth; ++Depth) {
    if (auto *Phi = dyn_cast<PHINode>(I))
      return Phi;
    if (!isProjection(I))
      return nullptr;
    auto *Inner = dyn_cast<Instruction>(I->getOperand(0));
    I = Inner && Inner->getParent() == I->getParent() ? Inner : nullptr;
  }
  return nullptr;
}

class Prover {
public:
  Prover(const DominatorTree &DT, const DataLayout &DL) : DT(DT), DL(DL) {}

  // Whether `pred lhs, rhs` is decided at the end of `block`.
  std::optional<bool> holdsAt(BasicBlock *Block, CmpPredicate Pred,
                              Value *LHS, Value *RHS) {
    Budget = MaxEdgesPerQuery;
    return holdsAtEnd(Block, Pred, LHS, RHS, 0, {});
  }

private:
  const DominatorTree &DT;
  const DataLayout &DL;
  unsigned Budget = 0;

  // A condition known to be `isTrue` where it is asked.
  struct Fact {
    Value *Cond;
    bool IsTrue;
  };
  using Facts = SmallVector<Fact, 8>;

  std::optional<bool> constantResult(CmpPredicate Pred, Value *LHS,
                                     Value *RHS) {
    if (Value *Folded = simplifyICmpInst(Pred, LHS, RHS, SimplifyQuery(DL)))
      if (auto *Constant = dyn_cast<ConstantInt>(Folded))
        return Constant->isOne();
    return std::nullopt;
  }

  std::optional<bool> impliedBy(Value *Cond, bool CondIsTrue,
                                CmpPredicate Pred, Value *LHS, Value *RHS) {
    if (!Cond->getType()->isIntegerTy(1))
      return std::nullopt;
    return isImpliedCondition(Cond, Pred, LHS, RHS, DL, CondIsTrue);
  }

  // A branch above `block` whose taken edge dominates it and implies the
  // comparison.
  std::optional<bool> impliedByDominatingBranch(BasicBlock *Block,
                                                CmpPredicate Pred, Value *LHS,
                                                Value *RHS) {
    const DomTreeNode *Node = DT.getNode(Block);
    for (unsigned Depth = 0; Node && Depth < MaxDominatorDepth; ++Depth) {
      const DomTreeNode *Parent = Node->getIDom();
      if (!Parent)
        break;
      BasicBlock *Dom = Parent->getBlock();
      if (auto *Br = dyn_cast<BranchInst>(Dom->getTerminator());
          Br && Br->isConditional() &&
          Br->getSuccessor(0) != Br->getSuccessor(1)) {
        for (unsigned Index = 0; Index < 2; ++Index) {
          BasicBlockEdge Edge(Dom, Br->getSuccessor(Index));
          if (!DT.dominates(Edge, Block))
            continue;
          if (auto Result =
                  impliedBy(Br->getCondition(), Index == 0, Pred, LHS, RHS))
            return Result;
        }
      }
      Node = Parent;
    }
    return std::nullopt;
  }

  // `value` as the edge from `from` into `merge` carries it, or null when
  // that value is not at hand there. A phi of `merge` carries its incoming
  // value. A value defined in a block that strictly dominates `merge`
  // dominates the end of every predecessor: it is the same on every edge. A
  // projection in `merge` carries the same operation on its operand's edge
  // value, when an identical instruction already computes that above the
  // edge; a slice's length read from a phi of slices is the usual case. Any
  // other value defined at or below the merge has no value on its edges.
  Value *valueOnEdge(Value *V, BasicBlock *Merge, BasicBlock *From,
                     unsigned Depth = 0) {
    auto *I = dyn_cast<Instruction>(V);
    if (!I)
      return V;
    if (I->getParent() != Merge)
      return DT.properlyDominates(I->getParent(), Merge) ? V : nullptr;
    if (auto *Phi = dyn_cast<PHINode>(I))
      return Phi->getIncomingValueForBlock(From);
    if (Depth >= MaxTranslationDepth)
      return nullptr;
    // A comparison in `merge` whose operands the edge carries as values
    // that decide it is that constant on the edge.
    if (auto *Cmp = dyn_cast<ICmpInst>(I)) {
      Value *L = valueOnEdge(Cmp->getOperand(0), Merge, From, Depth + 1);
      Value *R = valueOnEdge(Cmp->getOperand(1), Merge, From, Depth + 1);
      if (!L || !R)
        return nullptr;
      if (std::optional<bool> Result =
              constantResult(Cmp->getCmpPredicate(), L, R))
        return ConstantInt::getBool(Cmp->getContext(), *Result);
      return nullptr;
    }
    if (I->hasPoisonGeneratingFlags() || !isProjection(I))
      return nullptr;
    Value *Operand = valueOnEdge(I->getOperand(0), Merge, From, Depth + 1);
    if (!Operand || isa<Constant>(Operand))
      return nullptr;
    for (User *U : Operand->users()) {
      auto *Same = dyn_cast<Instruction>(U);
      if (!Same || Same == I || Same->getOperand(0) != Operand ||
          !Same->isSameOperationAs(I) || Same->hasPoisonGeneratingFlags() ||
          !DT.dominates(Same, From->getTerminator()))
        continue;
      return Same;
    }
    return nullptr;
  }

  // The merge to split on: the block of a phi that an operand projects and
  // that dominates `block`, nearest to it.
  BasicBlock *splitPoint(BasicBlock *Block, Value *LHS, Value *RHS) {
    BasicBlock *Best = nullptr;
    for (Value *Operand : {LHS, RHS}) {
      PHINode *Phi = projectedPhi(Operand);
      if (!Phi)
        continue;
      BasicBlock *Merge = Phi->getParent();
      if (!DT.dominates(Merge, Block) || !Merge->hasNPredecessorsOrMore(2))
        continue;
      if (!Best || DT.dominates(Best, Merge))
        Best = Merge;
    }
    return Best;
  }

  // The conditions of the branches whose taken edges lead from `merge` to
  // `block`, as they hold at the end of `block`, after `known`.
  Facts pathFacts(BasicBlock *Block, BasicBlock *Merge, const Facts &Known) {
    Facts Path(Known.begin(), Known.end());
    const DomTreeNode *Node = DT.getNode(Block);
    for (unsigned Depth = 0; Node && Node->getBlock() != Merge &&
                             Depth < MaxDominatorDepth;
         ++Depth) {
      const DomTreeNode *Parent = Node->getIDom();
      if (!Parent)
        break;
      BasicBlock *Dom = Parent->getBlock();
      if (auto *Br = dyn_cast<BranchInst>(Dom->getTerminator());
          Br && Br->isConditional() &&
          Br->getSuccessor(0) != Br->getSuccessor(1))
        for (unsigned Index = 0; Index < 2; ++Index)
          if (DT.dominates(BasicBlockEdge(Dom, Br->getSuccessor(Index)),
                           Block))
            Path.push_back(Fact{Br->getCondition(), Index == 0});
      Node = Parent;
    }
    return Path;
  }

  std::optional<bool> holdsAtEnd(BasicBlock *Block, CmpPredicate Pred,
                                 Value *LHS, Value *RHS, unsigned Depth,
                                 const Facts &Known) {
    if (!DT.isReachableFromEntry(Block))
      return std::nullopt;
    LHS = throughTrivialPhis(LHS);
    RHS = throughTrivialPhis(RHS);
    if (auto Result = constantResult(Pred, LHS, RHS))
      return Result;
    if (auto Result = impliedByDominatingBranch(Block, Pred, LHS, RHS))
      return Result;
    for (const Fact &Given : Known)
      if (auto Result = impliedBy(Given.Cond, Given.IsTrue, Pred, LHS, RHS))
        return Result;
    if (Depth >= MaxSplitDepth)
      return std::nullopt;
    BasicBlock *Merge = splitPoint(Block, LHS, RHS);
    if (!Merge)
      return std::nullopt;
    Facts Path = pathFacts(Block, Merge, Known);
    SmallVector<BasicBlock *, MaxPredecessors> Preds;
    for (BasicBlock *From : predecessors(Merge)) {
      if (is_contained(Preds, From))
        continue;
      if (Preds.size() == MaxPredecessors)
        return std::nullopt;
      Preds.push_back(From);
    }
    std::optional<bool> Agreed;
    for (BasicBlock *From : Preds) {
      if (Budget == 0)
        return std::nullopt;
      --Budget;
      std::optional<Facts> OnEdge = factsOnEdge(Path, Merge, From);
      if (!OnEdge)
        continue;
      Value *EdgeLHS = valueOnEdge(LHS, Merge, From);
      Value *EdgeRHS = valueOnEdge(RHS, Merge, From);
      if (!EdgeLHS || !EdgeRHS)
        return std::nullopt;
      std::optional<bool> Result = holdsOnEdge(From, Merge, Pred, EdgeLHS,
                                               EdgeRHS, Depth + 1, *OnEdge);
      if (!Result || (Agreed && *Agreed != *Result))
        return std::nullopt;
      Agreed = Result;
    }
    // When no edge can reach the block nothing is decided about it.
    return Agreed;
  }

  // `path` as the edge from `from` into `merge` carries it, or none when one
  // of its facts is false there, so that the edge cannot lead to where they
  // hold. A fact without a value on the edge is dropped, and one that the
  // edge decides as it should is no longer needed.
  std::optional<Facts> factsOnEdge(const Facts &Path, BasicBlock *Merge,
                                   BasicBlock *From) {
    Facts OnEdge;
    for (const Fact &Given : Path) {
      Value *Cond = valueOnEdge(Given.Cond, Merge, From);
      if (!Cond)
        continue;
      if (auto *Constant = dyn_cast<ConstantInt>(Cond)) {
        if (Constant->isOne() != Given.IsTrue)
          return std::nullopt;
        continue;
      }
      OnEdge.push_back(Fact{Cond, Given.IsTrue});
    }
    return OnEdge;
  }

  // The value `known` gives `cond`, directly or as a constant.
  std::optional<bool> knownValue(Value *Cond, const Facts &Known) {
    if (auto *Constant = dyn_cast<ConstantInt>(Cond))
      return Constant->isOne();
    for (const Fact &Given : Known)
      if (Given.Cond == Cond)
        return Given.IsTrue;
    return std::nullopt;
  }

  // Adds to `out` what `cond` being `isTrue` says given `known`: the
  // condition itself, and the operand of a logical and that is false, or of
  // a logical or that is true, whose other operand `known` decides so that
  // it alone gives the result.
  void resolve(Value *Cond, bool IsTrue, const Facts &Known, Facts &Out,
               unsigned Depth = 0) {
    using namespace PatternMatch;
    Out.push_back(Fact{Cond, IsTrue});
    Value *A = nullptr, *B = nullptr;
    if (Depth >= MaxResolutionDepth ||
        !(IsTrue ? match(Cond, m_LogicalOr(m_Value(A), m_Value(B)))
                : match(Cond, m_LogicalAnd(m_Value(A), m_Value(B)))))
      return;
    // `a or b` true with `a` false, or `a and b` false with `a` true, says
    // that `b` is as the whole is.
    if (knownValue(A, Known) == !IsTrue)
      resolve(B, IsTrue, Known, Out, Depth + 1);
    else if (knownValue(B, Known) == !IsTrue)
      resolve(A, IsTrue, Known, Out, Depth + 1);
  }

  // Whether `pred lhs, rhs` is decided on the edge from `from` into `to`:
  // by the edge's own branch, read with what is known there, or at the end
  // of `from`.
  std::optional<bool> holdsOnEdge(BasicBlock *From, BasicBlock *To,
                                  CmpPredicate Pred, Value *LHS, Value *RHS,
                                  unsigned Depth, const Facts &Known) {
    if (auto *Br = dyn_cast<BranchInst>(From->getTerminator());
        Br && Br->isConditional() &&
        Br->getSuccessor(0) != Br->getSuccessor(1)) {
      Facts OnEdge;
      resolve(Br->getCondition(), Br->getSuccessor(0) == To, Known, OnEdge);
      for (const Fact &Given : OnEdge)
        if (auto Result = impliedBy(Given.Cond, Given.IsTrue, Pred, LHS, RHS))
          return Result;
    }
    return holdsAtEnd(From, Pred, LHS, RHS, Depth, Known);
  }
};

// How far `SumProver` looks: how deeply a sum is taken apart into a base and
// bounded addends, how many dominating branches are consulted, and how
// deeply their conditions are taken apart into conjuncts.
constexpr unsigned MaxSumDepth = 4;
constexpr unsigned MaxFactDominators = 32;
constexpr unsigned MaxConjunctDepth = 2;

// Decides comparisons that ask whether a sum exceeds the largest value of its
// type, which is how a checked unsigned addition asks whether it overflows:
// `a + b` overflows when `a > ~b`, and `a + k` when `a > ~k`.
//
// A sum cannot exceed that largest value when it is at most some value of its
// type. Checked code says so in a form LLVM does not use: it asks that the
// room past a position be large enough, `n - p >= c`, and then adds to `p`
// offsets below `c`. LLVM's ConstraintElimination does not know that every
// value is at most the type's largest, and so never decides `p + k > ~0`.
//
// The prover takes the sum apart into a base and addends whose largest values
// are known, through additions that do not wrap, and looks for a branch that
// dominates the comparison and on its taken edge proves `base + c <= x` for
// some value `x`: `x - base >= c` by a subtraction that does not wrap,
// `base < x`, or `base <= x`. If the addends together with what the
// comparison adds are at most `c`, the sum is at most `x`.
class SumProver {
public:
  SumProver(const DominatorTree &DT, const DataLayout &DL) : DT(DT), DL(DL) {}

  // The value of `cmp` at its block when it asks whether a sum exceeds the
  // largest value and the sum is proven not to.
  std::optional<bool> decide(ICmpInst &Cmp) {
    std::optional<Question> Asked = question(Cmp);
    if (!Asked)
      return std::nullopt;
    SmallVector<Term, 8> Terms;
    split(Asked->Sum, APInt::getZero(Asked->Extra.getBitWidth()), 0, Terms);
    for (const Term &Candidate : Terms) {
      bool Overflow = false;
      APInt Need = Candidate.Offset.uadd_ov(Asked->Extra, Overflow);
      if (!Overflow && bounded(Cmp.getParent(), Candidate.Base, Need))
        return Asked->ValueIfBounded;
    }
    return std::nullopt;
  }

private:
  const DominatorTree &DT;
  const DataLayout &DL;

  // `sum + extra` exceeding the largest value of its type decides the
  // comparison; when it does not, the comparison is `valueIfBounded`.
  struct Question {
    Value *Sum;
    APInt Extra;
    bool ValueIfBounded;
  };

  // A value that is `base` plus addends at most `offset`.
  struct Term {
    Value *Base;
    APInt Offset;
  };

  // The largest value `v` can have. LLVM's range analysis does not look
  // through a zero extension, which is how a count of bits of a narrow mask
  // becomes an offset.
  APInt largest(Value *V, unsigned Depth = 0) {
    using namespace PatternMatch;
    APInt Max = computeConstantRangeIncludingKnownBits(V, /*ForSigned=*/false,
                                                       SimplifyQuery(DL))
                    .getUnsignedMax();
    Value *Narrow = nullptr;
    if (Depth < MaxSumDepth && match(V, m_ZExt(m_Value(Narrow))))
      Max = APIntOps::umin(
          Max, largest(Narrow, Depth + 1).zext(Max.getBitWidth()));
    return Max;
  }

  std::optional<Question> question(ICmpInst &Cmp) {
    using namespace PatternMatch;
    if (!Cmp.getOperand(0)->getType()->isIntegerTy())
      return std::nullopt;
    CmpPredicate Pred = Cmp.getCmpPredicate();
    Value *LHS = Cmp.getOperand(0);
    Value *RHS = Cmp.getOperand(1);
    // `~b < a` asks what `a > ~b` asks.
    if (Pred == ICmpInst::ICMP_ULT && match(LHS, m_Not(m_Value()))) {
      std::swap(LHS, RHS);
      Pred = ICmpInst::ICMP_UGT;
    }
    // `a > ~b`: `a + b` exceeds the largest value.
    Value *Addend = nullptr;
    if (Pred == ICmpInst::ICMP_UGT && match(RHS, m_Not(m_Value(Addend))))
      return Question{LHS, largest(Addend), false};
    const APInt *Bound = nullptr;
    if (!match(RHS, m_APInt(Bound)))
      return std::nullopt;
    // `sum > k` and `sum >= k + 1` are false, and `sum < k + 1` and
    // `sum <= k` true, when `sum + ~k` does not exceed the largest value.
    APInt Limit = *Bound;
    switch (Pred) {
    case ICmpInst::ICMP_UGT:
      return Question{LHS, ~Limit, false};
    case ICmpInst::ICMP_ULE:
      return Question{LHS, ~Limit, true};
    case ICmpInst::ICMP_UGE:
      if (Limit.isZero())
        return std::nullopt;
      return Question{LHS, ~(Limit - 1), false};
    case ICmpInst::ICMP_ULT:
      if (Limit.isZero())
        return std::nullopt;
      return Question{LHS, ~(Limit - 1), true};
    default:
      return std::nullopt;
    }
  }

  // Every way to write `v + offset` as a base plus bounded addends, taking
  // apart additions that do not wrap.
  void split(Value *V, APInt Offset, unsigned Depth,
             SmallVectorImpl<Term> &Terms) {
    using namespace PatternMatch;
    Terms.push_back(Term{V, Offset});
    Value *X = nullptr, *Y = nullptr;
    if (Depth >= MaxSumDepth || !match(V, m_NUWAddLike(m_Value(X), m_Value(Y))))
      return;
    for (auto [Base, Addend] : {std::pair{X, Y}, std::pair{Y, X}}) {
      bool Overflow = false;
      APInt Sum = Offset.uadd_ov(largest(Addend), Overflow);
      if (!Overflow)
        split(Base, Sum, Depth + 1, Terms);
    }
  }

  // Whether a branch dominating the end of `block` proves `base + need <= x`
  // for some value `x`.
  bool bounded(BasicBlock *Block, Value *Base, const APInt &Need) {
    const DomTreeNode *Node = DT.getNode(Block);
    for (unsigned Depth = 0; Node && Depth < MaxFactDominators; ++Depth) {
      const DomTreeNode *Parent = Node->getIDom();
      if (!Parent)
        break;
      BasicBlock *Dom = Parent->getBlock();
      if (auto *Br = dyn_cast<BranchInst>(Dom->getTerminator());
          Br && Br->isConditional() &&
          Br->getSuccessor(0) != Br->getSuccessor(1)) {
        for (unsigned Index = 0; Index < 2; ++Index) {
          BasicBlockEdge Edge(Dom, Br->getSuccessor(Index));
          if (DT.dominates(Edge, Block) &&
              proves(Br->getCondition(), Index == 0, Base, Need, 0))
            return true;
        }
      }
      Node = Parent;
    }
    return false;
  }

  // Whether `cond` having the value `isTrue` proves `base + need <= x` for
  // some value `x`.
  bool proves(Value *Cond, bool IsTrue, Value *Base, const APInt &Need,
              unsigned Depth) {
    using namespace PatternMatch;
    Value *A = nullptr, *B = nullptr;
    if (Depth < MaxConjunctDepth &&
        (IsTrue ? match(Cond, m_LogicalAnd(m_Value(A), m_Value(B)))
                : match(Cond, m_LogicalOr(m_Value(A), m_Value(B)))))
      return proves(A, IsTrue, Base, Need, Depth + 1) ||
             proves(B, IsTrue, Base, Need, Depth + 1);
    auto *Fact = dyn_cast<ICmpInst>(Cond);
    if (!Fact)
      return false;
    CmpPredicate Pred =
        IsTrue ? Fact->getCmpPredicate() : Fact->getInverseCmpPredicate();
    Value *LHS = Fact->getOperand(0);
    Value *RHS = Fact->getOperand(1);
    if (Pred == ICmpInst::ICMP_ULE || Pred == ICmpInst::ICMP_ULT) {
      std::swap(LHS, RHS);
      Pred = ICmpInst::getSwappedPredicate(Pred);
    }
    // `x - base >= c`: the subtraction does not wrap, or the branch would
    // be on poison, so `base + c <= x`.
    const APInt *Room = nullptr;
    if (match(LHS, m_NUWSub(m_Value(), m_Specific(Base))) &&
        match(RHS, m_APInt(Room))) {
      if (Pred == ICmpInst::ICMP_UGE)
        return Need.ule(*Room);
      if (Pred == ICmpInst::ICMP_UGT)
        return Need.ule(*Room) || (!Room->isMaxValue() && Need == *Room + 1);
      return false;
    }
    // `x > base` and `x >= base`.
    if (RHS == Base) {
      if (Pred == ICmpInst::ICMP_UGT)
        return Need.ule(1);
      if (Pred == ICmpInst::ICMP_UGE)
        return Need.isZero();
    }
    return false;
  }
};

struct MergeImpliedConditionsPass
    : PassInfoMixin<MergeImpliedConditionsPass> {
  PreservedAnalyses run(Function &F, FunctionAnalysisManager &AM) {
    if (F.hasOptNone())
      return PreservedAnalyses::all();
    const DominatorTree &DT = AM.getResult<DominatorTreeAnalysis>(F);
    Prover Prove(DT, F.getDataLayout());
    bool Changed = false;
    for (BasicBlock &BB : F) {
      if (!DT.isReachableFromEntry(&BB))
        continue;
      for (Instruction &I : make_early_inc_range(BB)) {
        auto *Cmp = dyn_cast<ICmpInst>(&I);
        if (!Cmp || Cmp->use_empty() || Cmp->getType()->isVectorTy())
          continue;
        // Only a comparison of a phi can be decided by a split; any other is
        // left to LLVM, which already uses the dominating conditions.
        if (!projectedPhi(Cmp->getOperand(0)) &&
            !projectedPhi(Cmp->getOperand(1)))
          continue;
        // A comparison's operands dominate it, so they have their values at
        // its block's end as well, which is where the question is asked.
        if (std::optional<bool> Value =
                Prove.holdsAt(&BB, Cmp->getCmpPredicate(), Cmp->getOperand(0),
                              Cmp->getOperand(1))) {
          Cmp->replaceAllUsesWith(
              ConstantInt::getBool(Cmp->getContext(), *Value));
          Cmp->eraseFromParent();
          Changed = true;
        }
      }
    }
    if (!Changed)
      return PreservedAnalyses::all();
    // Only uses of comparisons changed; the control flow is folded later.
    PreservedAnalyses Preserved;
    Preserved.preserveSet<CFGAnalyses>();
    return Preserved;
  }
};

// Decides the comparisons `SumProver` decides.
struct BoundedSumsPass : PassInfoMixin<BoundedSumsPass> {
  PreservedAnalyses run(Function &F, FunctionAnalysisManager &AM) {
    if (F.hasOptNone())
      return PreservedAnalyses::all();
    const DominatorTree &DT = AM.getResult<DominatorTreeAnalysis>(F);
    SumProver Prove(DT, F.getDataLayout());
    bool Changed = false;
    for (BasicBlock &BB : F) {
      if (!DT.isReachableFromEntry(&BB))
        continue;
      for (Instruction &I : make_early_inc_range(BB)) {
        auto *Cmp = dyn_cast<ICmpInst>(&I);
        if (!Cmp || Cmp->use_empty() || Cmp->getType()->isVectorTy())
          continue;
        if (std::optional<bool> Value = Prove.decide(*Cmp)) {
          Cmp->replaceAllUsesWith(
              ConstantInt::getBool(Cmp->getContext(), *Value));
          Cmp->eraseFromParent();
          Changed = true;
        }
      }
    }
    if (!Changed)
      return PreservedAnalyses::all();
    PreservedAnalyses Preserved;
    Preserved.preserveSet<CFGAnalyses>();
    return Preserved;
  }
};

} // namespace

// The passes run twice. First where the function simplification pipeline
// ends, after inlining and loop rotation have shaped the merges, and before
// the CFG cleanup that removes the branches they fold. Then once more where
// the module pipeline ends, followed by a CFG cleanup of its own: the loop
// passes and the vectorizers in between rewrite the merges and offsets that
// the first run left, and expose facts it could not see, such as a check that
// a later pass moved below the branch proving it.
//
// Every pass builder of the bridges registers them: those of objects, of
// ThinLTO and full LTO pre-link bitcode, and of the ThinLTO backend after the
// thin link, where imported functions are inlined (llvm_thin_backend.cpp).
// Full LTO's post-link pipeline is LLVM's own, since LLVM's regular LTO driver
// has no way to add passes in process. LLVM invokes these callbacks at O0 as
// well, where nothing is optimized, so they add nothing there.
void niaRegisterPasses(PassBuilder &PB) {
  PB.registerScalarOptimizerLateEPCallback(
      [](FunctionPassManager &FPM, OptimizationLevel Level) {
        if (Level == OptimizationLevel::O0)
          return;
        FPM.addPass(MergeImpliedConditionsPass());
        FPM.addPass(BoundedSumsPass());
      });
  PB.registerOptimizerLastEPCallback(
      [](ModulePassManager &MPM, OptimizationLevel Level, ThinOrFullLTOPhase) {
        if (Level == OptimizationLevel::O0)
          return;
        FunctionPassManager FPM;
        FPM.addPass(MergeImpliedConditionsPass());
        FPM.addPass(BoundedSumsPass());
        FPM.addPass(SimplifyCFGPass());
        MPM.addPass(createModuleToFunctionPassAdaptor(std::move(FPM)));
      });
}
