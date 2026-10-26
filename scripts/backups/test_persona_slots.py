import unittest
from copy import deepcopy
import backup


class PersonaCapacity(unittest.TestCase):
    def test_preset_grant_survives_without_credentials_but_is_not_a_human_exemption(self):
        game = {'accounts': [{'id': 1, 'character_slots': 14}], 'characters': [],
                'character_slot_purchases': [], 'shore_bots': [{'account': 1, 'name': 'persona:v1:Acrobat'}]}
        backup.validate_slots(game)
        for change in ['missing_registry', 'old_bot', 'wrong_capacity', 'extra_receipt']:
            wrong = deepcopy(game)
            if change == 'missing_registry':
                wrong['shore_bots'] = []
            elif change == 'old_bot':
                wrong['shore_bots'][0]['name'] = 'Bayleaf'
            elif change == 'wrong_capacity':
                wrong['accounts'][0]['character_slots'] = 13
            else:
                wrong['character_slot_purchases'] = [{'account': 1, 'timer': -2, 'slots': 2}]
            with self.subTest(change=change), self.assertRaises(ValueError):
                backup.validate_slots(wrong)
