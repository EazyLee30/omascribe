import copy
import unittest
from compare_incremental_minutes import promotion_gate


class IncrementalCostTests(unittest.TestCase):
    def test_failed_quality_missing_arms_and_unknown_cost_cannot_promote(self):
        legacy=dict(mode='legacy',exit_code=0,report=dict(passed=True),backend_evidence=dict(passed=True),summary=dict(input_tokens=1000,output_tokens=200,process_seconds=10))
        delta=dict(legacy,mode='incremental',summary=dict(input_tokens=800,output_tokens=200,process_seconds=9))
        self.assertTrue(promotion_gate([legacy,delta]))
        for field in ['input_tokens','output_tokens','process_seconds']:
            bad=copy.deepcopy(delta);bad['summary'][field]=None
            self.assertFalse(promotion_gate([legacy,bad]))
        for change in [dict(report=dict(passed=False)),dict(exit_code=1),dict(backend_evidence=dict(passed=False)),dict(summary=dict(input_tokens=800,output_tokens=500,process_seconds=9)),dict(summary=dict(input_tokens=800,output_tokens=200,process_seconds=11))]:
            self.assertFalse(promotion_gate([legacy,dict(delta,**change)]))
        self.assertFalse(promotion_gate([legacy]))
        self.assertFalse(promotion_gate([delta,delta]))
